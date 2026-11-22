//! `ns-next mcp`, the Model Context Protocol server through which an agent
//! debugs a program (`docs/design/debugger.md`, "The MCP server"): each
//! test starts the server as a child and speaks JSON-RPC to it, one
//! message a line, as an MCP client does.

mod common;

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::ScratchDirectory;
use indoc::indoc;
use serde_json::{Value, json};

/// The longest any single answer takes before the test fails.
const TIMEOUT: Duration = Duration::from_secs(60);

/// The stress modes a plain server strips from its programs' environment.
const STRESS_MODES: [&str; 15] = [
    "MOVES",
    "COLLECT",
    "SCAVENGE",
    "SUSPEND",
    "ZONE",
    "JIT",
    "EXIT",
    "OPTIMIZE",
    "DEOPT",
    "CACHES",
    "INVALIDATE",
    "WIDE",
    "PAUSE",
    "BREAKPOINTS",
    "STEP",
];

/// The tools and their parameters, the required ones first.
const TOOLS: [(&str, &[&str], &[&str]); 17] = [
    (
        "debug_start",
        &["program"],
        &[
            "breakpoints",
            "stop_on_entry",
            "arguments",
            "runtime",
            "trace",
            "trace_filter",
            "wait",
        ],
    ),
    ("debug_breakpoints", &[], &["set", "clear", "session"]),
    ("debug_continue", &[], &["wait", "session"]),
    ("debug_step", &["kind"], &["target", "wait", "session"]),
    ("debug_step_targets", &[], &["session"]),
    ("debug_run_to", &["location"], &["wait", "session"]),
    ("debug_pause", &[], &["session"]),
    ("debug_wait", &["seconds"], &["session"]),
    ("debug_stack", &[], &["session"]),
    ("debug_code", &[], &["frame", "session"]),
    (
        "debug_variables",
        &[],
        &["frame", "reference", "start", "count", "session"],
    ),
    ("debug_evaluate", &["expression"], &["frame", "session"]),
    (
        "debug_set_variable",
        &["name", "value"],
        &["frame", "session"],
    ),
    ("debug_frame", &["action", "frame"], &["value", "session"]),
    ("debug_memory", &[], &["class", "referrers_of", "session"]),
    ("debug_mark", &["label"], &["reference", "session"]),
    ("debug_stop", &[], &["session"]),
];

/// A running `ns-next mcp` and the lines it writes to its standard output.
/// Its input is closed once the process started ends, as Node's
/// `child_process` closes it, on which the MCP SDKs' stdio transports are
/// built: a server that handed over to another process would lose its
/// input then.
struct McpServer {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    lines: Receiver<String>,
    stderr: Arc<Mutex<Vec<u8>>>,
    next_id: i64,
    /// The responses read while another was looked for, by their id.
    kept: HashMap<i64, Value>,
}

impl McpServer {
    /// `ns-next mcp`, initialized.
    fn start() -> McpServer {
        McpServer::start_with_environment(&[])
    }

    /// As `start`, with the environment `variables`.
    fn start_with_environment(variables: &[(&str, &str)]) -> McpServer {
        McpServer::start_with(Path::new(binary()), variables)
    }

    /// `<executable> mcp` with the environment `variables`, initialized.
    fn start_with(executable: &Path, variables: &[(&str, &str)]) -> McpServer {
        McpServer::launch(executable, variables, false)
    }

    /// As `start`, with neither a stress mode nor `NS_NEXT_NO_JIT` in the
    /// environment its programs inherit, whatever the test run says, so
    /// that machine code compiles what a plain run's thresholds decide.
    fn start_plain() -> McpServer {
        McpServer::launch(Path::new(binary()), &[], true)
    }

    fn launch(executable: &Path, variables: &[(&str, &str)], plain: bool) -> McpServer {
        let mut command = common::target_command(executable);
        command
            .arg("mcp")
            .envs(variables.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if plain {
            command.env_remove("NS_NEXT_NO_JIT");
            for mode in STRESS_MODES {
                command.env_remove(format!("NS_NEXT_STRESS_{mode}"));
            }
        }
        let mut child = command.spawn().expect("start ns-next mcp");
        let stdout = child.stdout.take().expect("a piped stdout");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let mut reader = child.stderr.take().expect("a piped stderr");
        let filled = Arc::clone(&stderr);
        std::thread::spawn(move || {
            let mut chunk = [0; 4096];
            while let Ok(read) = reader.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                filled
                    .lock()
                    .expect("stderr")
                    .extend_from_slice(&chunk[..read]);
            }
        });
        let stdin = Arc::new(Mutex::new(child.stdin.take()));
        let child = Arc::new(Mutex::new(child));
        let watched = Arc::clone(&child);
        let closed = Arc::clone(&stdin);
        std::thread::spawn(move || {
            loop {
                let ended = watched
                    .lock()
                    .expect("the child")
                    .try_wait()
                    .map_or(true, |status| status.is_some());
                if ended {
                    closed.lock().expect("the input").take();
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let mut server = McpServer {
            child,
            stdin,
            lines,
            stderr,
            next_id: 1,
            kept: HashMap::new(),
        };
        let initialized = server.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "ns-next tests", "version": "1" },
            }),
        );
        assert_eq!(
            initialized["result"]["protocolVersion"], "2025-06-18",
            "{initialized}"
        );
        server.notify("notifications/initialized");
        server
    }

    fn write(&mut self, message: &Value) {
        let mut stdin = self.stdin.lock().expect("the input");
        let stdin = stdin
            .as_mut()
            .expect("the server's input, closed once the process started ended");
        writeln!(stdin, "{message}").expect("write to the server");
        stdin.flush().expect("flush the server's input");
    }

    /// The process id of the program the `start`th `debug_start` started,
    /// counting from 1, as the server says it on its standard error. The
    /// server says it before the start answers, but the reader of the
    /// standard error may take it in later than the reader of the output
    /// takes the answer, so the line is waited for by its place, never
    /// taken as the last one there.
    fn program_process(&self, start: usize) -> u32 {
        let started = Instant::now();
        loop {
            let stderr = String::from_utf8_lossy(&self.stderr.lock().expect("stderr")).to_string();
            let process = stderr
                .lines()
                .filter_map(|line| line.split(" runs as process ").nth(1))
                .nth(start - 1)
                .map(|line| line.trim().parse().expect("a process id"));
            if let Some(process) = process {
                return process;
            }
            assert!(
                started.elapsed() < TIMEOUT,
                "no process of start {start} on stderr: {stderr}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Sends the notification that the client cancelled its request
    /// `identifier`.
    fn cancel(&mut self, identifier: i64) {
        self.write(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": { "requestId": identifier, "reason": "the user interrupted it" },
        }));
    }

    /// The response to `identifier`, which must come within `TIMEOUT`, as
    /// its text and whether it is an error.
    fn answered(&mut self, identifier: i64, what: &str) -> (String, bool) {
        let response = self
            .response(identifier, TIMEOUT)
            .unwrap_or_else(|| panic!("{what} was not answered within {TIMEOUT:?}"));
        text_of(&response)
    }

    /// Closes the server's input, as a client that ends does.
    fn close_input(&self) {
        self.stdin.lock().expect("the input").take();
    }

    /// Whether the server's process ends within `within`.
    fn exited_within(&self, within: Duration) -> bool {
        let started = Instant::now();
        loop {
            let ended = self
                .child
                .lock()
                .expect("the child")
                .try_wait()
                .map_or(true, |status| status.is_some());
            if ended {
                return true;
            }
            if started.elapsed() > within {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn notify(&mut self, method: &str) {
        self.write(&json!({ "jsonrpc": "2.0", "method": method }));
    }

    /// Sends a request and answers its id, without waiting for its
    /// response.
    fn send(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    /// Sends a `tools/call` of `tool` without waiting; answers its id.
    fn send_call(&mut self, tool: &str, arguments: Value) -> i64 {
        self.send(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        )
    }

    /// The response to the request `identifier`, waiting `within` at most;
    /// every line the server writes must be a JSON-RPC message, and one
    /// that answers another request is kept for it.
    fn response(&mut self, identifier: i64, within: Duration) -> Option<Value> {
        if let Some(message) = self.kept.remove(&identifier) {
            return Some(message);
        }
        let deadline = Instant::now() + within;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let line = self.lines.recv_timeout(remaining).ok()?;
            let message = message_of(&line);
            if message["id"] == identifier {
                return Some(message);
            }
            let other = message["id"]
                .as_i64()
                .unwrap_or_else(|| panic!("a message answering no request: {message}"));
            self.kept.insert(other, message);
        }
    }

    /// Sends a request and answers its response.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        self.response(id, TIMEOUT).unwrap_or_else(|| {
            panic!(
                "no answer to `{method}` within {TIMEOUT:?}; stderr: {}",
                String::from_utf8_lossy(&self.stderr.lock().expect("stderr"))
            )
        })
    }

    /// The next line the server writes, as a message.
    fn next_message(&mut self) -> Value {
        let line = self
            .lines
            .recv_timeout(TIMEOUT)
            .unwrap_or_else(|_| panic!("no message within {TIMEOUT:?}"));
        message_of(&line)
    }

    /// Calls `tool`; answers its text and whether it is an error.
    fn call(&mut self, tool: &str, arguments: Value) -> (String, bool) {
        let response = self.request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        );
        text_of(&response)
    }

    /// Calls `tool`, which must succeed; answers its text.
    fn succeed(&mut self, tool: &str, arguments: Value) -> String {
        let (text, is_error) = self.call(tool, arguments);
        assert!(!is_error, "`{tool}` failed: {text}");
        text
    }

    /// Calls `tool`, which must fail; answers its reason.
    fn fail(&mut self, tool: &str, arguments: Value) -> String {
        let (text, is_error) = self.call(tool, arguments);
        assert!(is_error, "`{tool}` succeeded: {text}");
        text
    }
}

impl Drop for McpServer {
    /// Closing the input ends the server, which ends its program; a
    /// failing test leaves neither behind.
    fn drop(&mut self) {
        self.stdin.lock().expect("the input").take();
        let started = Instant::now();
        // The output ends once the server has exited.
        while started.elapsed() < TIMEOUT {
            match self.lines.recv_timeout(TIMEOUT) {
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let mut child = self.child.lock().expect("the child");
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// A line the server wrote, which must be a JSON-RPC message or a batch
/// of them.
fn message_of(line: &str) -> Value {
    let message: Value = serde_json::from_str(line)
        .unwrap_or_else(|error| panic!("not a JSON-RPC message ({error}): {line}"));
    let members = match &message {
        Value::Array(members) => members.clone(),
        single => vec![single.clone()],
    };
    for member in members {
        assert_eq!(member["jsonrpc"], "2.0", "{member}");
    }
    message
}

/// A `tools/call`'s response as its text and whether it is an error.
fn text_of(response: &Value) -> (String, bool) {
    let result = &response["result"];
    let content = result["content"]
        .as_array()
        .unwrap_or_else(|| panic!("{response}"));
    assert_eq!(content.len(), 1, "{response}");
    assert_eq!(content[0]["type"], "text", "{response}");
    let text = content[0]["text"].as_str().expect("a text").to_string();
    let is_error = result["isError"].as_bool().expect("isError");
    (text, is_error)
}

/// The session a `debug_start`'s answer names on its first line, and the
/// rest of the answer.
fn started(answer: &str) -> (u32, &str) {
    let (first, rest) = answer.split_once('\n').unwrap_or((answer, ""));
    let number = first
        .strip_prefix("session: ")
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("no session's number first:\n{answer}"));
    (number, rest)
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_ns-next")
}

/// A scratch directory holding `source` as `<name>.ns`, and the file's
/// absolute path.
fn program(name: &str, source: &str) -> (ScratchDirectory, PathBuf) {
    let directory = ScratchDirectory::new(name);
    let path = directory.join(&format!("{name}.ns"));
    std::fs::write(&path, source).expect("write the program");
    let path = std::path::absolute(&path).expect("an absolute path");
    (directory, path)
}

fn location(path: &Path, line: u32) -> String {
    format!("{}:{line}", path.display())
}

/// The line of `text` that starts with `prefix`.
fn line_starting<'a>(text: &'a str, prefix: &str) -> &'a str {
    text.lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no line starting {prefix:?}:\n{text}"))
}

/// The `n` of the first `[ref n]` after `after` in `text`.
fn reference_after(text: &str, after: &str) -> i64 {
    let start = text
        .find(after)
        .unwrap_or_else(|| panic!("no {after:?}:\n{text}"))
        + after.len();
    let rest = &text[start..];
    let begin = rest
        .find("[ref ")
        .unwrap_or_else(|| panic!("no reference:\n{text}"))
        + "[ref ".len();
    let end = rest[begin..].find(']').expect("a closing bracket") + begin;
    rest[begin..end].parse().expect("a reference number")
}

/// A root that prints, holds `count` at 41 and prints it.
const COUNTER: &str = indoc! {"
    class Counter = Object new () (
      public main: platform args: arguments = (
        | count ::= 41. out = platform transcript. |
        out show: 'start'; newLine.
        out show: 'count is'; newLine.
        out show: count printString; newLine.
      )
    ) : ()
"};

#[test]
fn the_server_lists_its_tools() {
    let mut server = McpServer::start();
    let listed = server.request("tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().expect("the tools");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("a name"))
        .collect();
    let expected: Vec<&str> = TOOLS.iter().map(|(name, _, _)| *name).collect();
    assert_eq!(names, expected);
    for (tool, (name, required, optional)) in tools.iter().zip(TOOLS) {
        assert!(
            tool["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "{tool}"
        );
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{tool}");
        assert_eq!(schema["required"], json!(required), "{name}");
        let mut properties: Vec<&str> = schema["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .map(String::as_str)
            .collect();
        properties.sort_unstable();
        let mut parameters: Vec<&str> = required.iter().chain(optional).copied().collect();
        parameters.sort_unstable();
        assert_eq!(properties, parameters, "{name}");
    }
    let unknown = server.request("resources/list", json!({}));
    assert_eq!(unknown["error"]["code"], -32601, "{unknown}");
    let ping = server.request("ping", json!({}));
    assert_eq!(ping["result"], json!({}), "{ping}");
}

#[test]
fn a_start_stops_at_a_breakpoint_with_a_summary() {
    let (_directory, path) = program("Counter", COUNTER);
    let mut server = McpServer::start();
    let answer = server.succeed(
        "debug_start",
        json!({
            "program": path,
            "breakpoints": [{ "location": location(&path, 5) }],
        }),
    );
    let (session, summary) = started(&answer);
    assert_eq!(session, 1, "{answer}");
    let first = summary.lines().next().expect("a second line");
    assert!(
        first.starts_with(&format!("stopped: breakpoint at {}:5:", path.display())),
        "{answer}"
    );
    assert!(
        first.ends_with("in Counter>>main:args: (frame 0)"),
        "{answer}"
    );
    assert_eq!(
        line_starting(&answer, ">").trim_end(),
        "> 5 |     out show: 'count is'; newLine.",
        "{answer}"
    );
    assert!(
        line_starting(&answer, "locals:").contains("count = 41"),
        "{answer}"
    );
    // Panics without the line.
    line_starting(&answer, "stack: #0 Counter>>main:args: Counter.ns:5");
    assert_eq!(line_starting(&answer, "output:"), "output: \"start\\n\"");
    assert!(line_starting(&answer, "runtime: ").len() > "runtime: ".len());
}

#[test]
fn a_step_and_an_evaluation_in_the_same_state() {
    let (_directory, path) = program("Counter", COUNTER);
    let mut server = McpServer::start();
    server.succeed(
        "debug_start",
        json!({
            "program": path,
            "breakpoints": [{ "location": location(&path, 5) }],
        }),
    );
    let stepped = server.succeed("debug_step", json!({ "kind": "over" }));
    assert!(
        stepped.starts_with(&format!("stopped: step at {}:6:", path.display())),
        "{stepped}"
    );
    assert_eq!(
        line_starting(&stepped, "output:"),
        "output: \"count is\\n\""
    );
    let evaluated = server.succeed("debug_evaluate", json!({ "expression": "count + 1" }));
    assert_eq!(evaluated, "42");
    let set = server.succeed(
        "debug_set_variable",
        json!({ "name": "count", "value": "count * 2" }),
    );
    assert_eq!(set, "count = 82");
    let ended = server.succeed("debug_continue", json!({}));
    assert_eq!(ended, "ended: exit status 0\noutput: \"82\\n\"");
    let refused = server.fail("debug_stack", json!({}));
    assert!(refused.contains("has ended"), "{refused}");
}

#[test]
fn debug_code_answers_the_bytecode() {
    let (_directory, path) = program("Counter", COUNTER);
    let mut server = McpServer::start();
    server.succeed(
        "debug_start",
        json!({
            "program": path,
            "breakpoints": [{ "location": location(&path, 5) }],
        }),
    );
    let code = server.succeed("debug_code", json!({}));
    assert!(
        code.starts_with("--- bytecode {Counter>>main:args:}\n"),
        "{code}"
    );
    assert!(
        code.lines().any(|line| line
            .split_whitespace()
            .next()
            .is_some_and(|word| word.ends_with("->") && word.len() > 2)),
        "{code}"
    );
    // The top frame stands at the stop's safepoint, never at a call its
    // machine code returns from.
    assert!(
        code.contains("--- end\nthe frame goes on in the interpreter"),
        "{code}"
    );
    let refused = server.fail("debug_code", json!({ "frame": 9 }));
    assert!(!refused.is_empty(), "{refused}");
}

/// A root that says it spins, then counts forever.
const SPIN: &str = indoc! {"
    class Spin = Object new () (
      public main: platform args: arguments = (
        | count ::= 0. |
        platform transcript show: 'spinning'; newLine.
        [true] whileTrue: [count:: count + 1].
      )
    ) : ()
"};

#[test]
fn an_endless_loop_answers_running_and_pauses_where_it_is() {
    let (_directory, path) = program("Spin", SPIN);
    let mut server = McpServer::start();
    let entry = server.succeed(
        "debug_start",
        json!({ "program": path, "stop_on_entry": true }),
    );
    assert!(
        started(&entry).1.starts_with("stopped: entry at "),
        "{entry}"
    );
    let running = server.succeed("debug_continue", json!({ "wait": 1 }));
    assert!(running.starts_with("running for "), "{running}");
    assert!(running.contains("output: \"spinning\\n\""), "{running}");
    let paused = server.succeed("debug_pause", json!({}));
    assert!(
        paused.starts_with(&format!("stopped: pause at {}:5:", path.display())),
        "{paused}"
    );
    let stopped = server.succeed("debug_stop", json!({}));
    assert!(stopped.starts_with("ended: exit status "), "{stopped}");
}

/// A root that sends `do:` to an array, which the shipped kernel answers.
const EACH: &str = indoc! {"
    class Each = Object new () (
      public main: platform args: arguments = (
        {1. 2} do: [:each | platform transcript show: each printString; newLine].
      )
    ) : ()
"};

#[test]
fn a_shipped_frame_names_a_file_with_the_binary_s_text() {
    let (directory, path) = program("Each", EACH);
    let mut server = McpServer::start();
    let answer = server.succeed(
        "debug_start",
        json!({
            "program": path,
            "breakpoints": [{ "location": "newspeak.core.Kernel/Array>>do:" }],
        }),
    );
    let first = started(&answer).1.lines().next().expect("a stop");
    let place = first
        .strip_prefix("stopped: function breakpoint at ")
        .and_then(|rest| rest.split(" in ").next())
        .unwrap_or_else(|| panic!("{answer}"));
    let mut parts = place.rsplitn(3, ':');
    let (_column, _line) = (parts.next(), parts.next());
    let kernel = PathBuf::from(parts.next().expect("a path"));
    assert!(kernel.is_absolute(), "{answer}");
    assert_eq!(
        kernel.file_name(),
        Some(OsStr::new("Kernel.ns")),
        "{answer}"
    );
    let shown = std::fs::read_to_string(&kernel).expect("the shown file");
    let written = directory.join("written");
    let (ok, _, error) = common::ns_next(&[OsStr::new("shipped-sources"), written.as_os_str()]);
    assert!(ok, "{error}");
    let shipped = std::fs::read_to_string(written.join("Kernel.ns")).expect("the shipped kernel");
    assert!(shown == shipped, "the shown Kernel.ns is not the binary's");
    let stack = server.succeed("debug_stack", json!({}));
    assert!(
        line_starting(&stack, "#0 ").contains(&kernel.display().to_string()),
        "{stack}"
    );
}

/// A root whose nested class `Account` takes a deposit.
const DEMO: &str = indoc! {"
    class Demo = Object new () (
      public main: platform args: arguments = (
        | account = Account new. |
        account deposit: 5.
        platform transcript show: account balance printString; newLine.
      )
      public class Account = Object new (
        | public balance ::= 0. |
      ) (
        public deposit: amount = (
          balance:: balance + amount
        )
      )
    ) : ()
"};

/// A method location names a class inside a module after a `/`, and the
/// stop's summary names the frame so; the form before, a dot, is refused
/// with the form it means.
#[test]
fn a_method_location_names_a_nested_class_with_a_slash() {
    let (_directory, path) = program("Demo", DEMO);
    let mut server = McpServer::start();
    let answer = server.succeed(
        "debug_start",
        json!({
            "program": path,
            "breakpoints": [{ "location": "Demo/Account>>deposit:" }],
        }),
    );
    let first = started(&answer).1.lines().next().expect("a stop");
    assert!(
        first.starts_with(&format!(
            "stopped: function breakpoint at {}:",
            path.display()
        )),
        "{answer}"
    );
    assert!(
        first.ends_with("in Demo/Account>>deposit: (frame 0)"),
        "{answer}"
    );
    line_starting(&answer, "stack: #0 Demo/Account>>deposit: Demo.ns:");
    server.succeed("debug_stop", json!({}));
    let refused = server.succeed(
        "debug_start",
        json!({
            "program": path,
            "breakpoints": [{ "location": "Demo.Account>>deposit:" }],
        }),
    );
    assert!(
        refused.contains(
            "no module `Demo.Account`; a class inside a module is written `Demo/Account`"
        ),
        "{refused}"
    );
}

#[test]
fn a_location_naming_no_module_is_refused() {
    let (directory, path) = program("Counter", COUNTER);
    let stranger = directory.join("Stranger.ns");
    std::fs::write(&stranger, COUNTER).expect("write a file of no module");
    let stranger = std::path::absolute(&stranger).expect("an absolute path");
    let mut server = McpServer::start();
    server.succeed(
        "debug_start",
        json!({ "program": path, "stop_on_entry": true }),
    );
    let refused = server.fail(
        "debug_breakpoints",
        json!({ "set": [{ "location": location(&stranger, 3) }] }),
    );
    assert!(
        refused.contains("not a module of this program"),
        "{refused}"
    );
    let listed = server.succeed("debug_breakpoints", json!({}));
    assert_eq!(listed, "no breakpoints");
    let relative = server.fail(
        "debug_breakpoints",
        json!({ "set": [{ "location": "Counter.ns:3" }] }),
    );
    assert!(relative.contains("absolute"), "{relative}");
}

/// A `debug_start` that names no runtime runs the program with the
/// server's own executable, wherever it lies, and the server leaves its
/// directory as it found it: it renames, copies and writes nothing there.
#[test]
fn a_start_without_a_runtime_runs_the_server_s_own_executable() {
    let build = ScratchDirectory::new("server");
    let name = Path::new(binary())
        .file_name()
        .expect("a file name")
        .to_string_lossy()
        .to_string();
    let executable = std::path::absolute(build.join(&name)).expect("an absolute path");
    std::fs::copy(binary(), &executable).expect("copy the binary");
    let mut server = McpServer::start_with(&executable, &[]);
    let (_directory, path) = program("Counter", COUNTER);
    let ended = server.succeed("debug_start", json!({ "program": path }));
    assert!(
        started(&ended).1.starts_with("ended: exit status 0"),
        "{ended}"
    );
    let runtime = line_starting(&ended, "runtime: ");
    assert!(
        runtime.contains(&executable.display().to_string()),
        "{ended}"
    );
    drop(server);
    let entries: Vec<String> = std::fs::read_dir(build.path())
        .expect("read the server's directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries, [name], "the server changed its directory");
}

/// A server that starts removes the directories of shipped sources in the
/// user's own directory untouched for more than a day, but the one of its
/// own binary's fingerprint, and keeps those touched since.
#[test]
fn a_starting_server_removes_stale_shipped_sources_but_its_own() {
    let data = ScratchDirectory::new("local-data");
    let root = data.path().join("ns-next").join("core-library");
    let running = ns_runtime::shipped_sources_fingerprint();
    let two_days_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
    let sources = |fingerprint: &str, touched: std::time::SystemTime| {
        let directory = root.join(fingerprint);
        std::fs::create_dir_all(&directory).expect("create the sources' directory");
        let file = directory.join("fingerprint");
        std::fs::write(&file, format!("{fingerprint}\n")).expect("write the fingerprint");
        std::fs::File::options()
            .write(true)
            .open(&file)
            .and_then(|file| file.set_modified(touched))
            .expect("age the fingerprint");
        directory
    };
    let stale = sources("0123456789abcdef", two_days_ago);
    let own = sources(&running, two_days_ago);
    let fresh = sources("fedcba9876543210", std::time::SystemTime::now());
    let data_path = data.path().display().to_string();
    let server = McpServer::start_with_environment(&[
        ("LOCALAPPDATA", data_path.as_str()),
        ("XDG_CACHE_HOME", data_path.as_str()),
    ]);
    assert!(!stale.exists(), "the stale sources are still there");
    assert!(own.is_dir(), "the binary's own sources were removed");
    assert!(fresh.is_dir(), "sources touched since were removed");
    drop(server);
}

/// Whether a process of the id `process` runs.
fn process_runs(process: u32) -> bool {
    if cfg!(windows) {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {process}"), "/NH", "/FO", "CSV"])
            .output()
            .expect("run tasklist");
        String::from_utf8_lossy(&output.stdout).contains(&format!("\"{process}\""))
    } else {
        std::process::Command::new("kill")
            .args(["-0", &process.to_string()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

/// A server killed outright drops its program's connection, and the
/// program, whose launcher is gone, ends.
#[test]
fn killing_the_server_ends_its_program() {
    let (_directory, path) = program("Spin", SPIN);
    let mut server = McpServer::start();
    let running = server.succeed("debug_start", json!({ "program": path, "wait": 0.5 }));
    assert!(started(&running).1.starts_with("running for "), "{running}");
    let process = server.program_process(1);
    assert!(process_runs(process), "the program does not run");
    {
        let mut child = server.child.lock().expect("the child");
        child.kill().expect("kill the server");
        let _ = child.wait();
    }
    let started = Instant::now();
    while process_runs(process) {
        assert!(
            started.elapsed() < TIMEOUT,
            "the program outlived its server"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A root that holds an array, then lets it go.
const MARKS: &str = indoc! {"
    class Marks = Object new () (
      public main: platform args: arguments = (
        | elements |
        elements:: {10. 20. 30}.
        platform transcript show: elements size printString; newLine.
      )
    ) : ()
"};

#[test]
fn a_reference_expands_and_a_mark_names_a_value() {
    let (_directory, path) = program("Marks", MARKS);
    let mut server = McpServer::start();
    let answer = server.succeed(
        "debug_start",
        json!({
            "program": path,
            "breakpoints": [{ "location": location(&path, 5) }],
        }),
    );
    let locals = line_starting(&answer, "locals:");
    let reference = reference_after(locals, "elements = ");
    let value = locals
        .split("elements = ")
        .nth(1)
        .expect("elements")
        .split(" [ref ")
        .next()
        .expect("a value")
        .to_string();
    let children = server.succeed("debug_variables", json!({ "reference": reference }));
    assert_eq!(children, "[1] = 10\n[2] = 20\n[3] = 30");
    server.succeed(
        "debug_mark",
        json!({ "reference": reference, "label": "first" }),
    );
    let marked = server.succeed(
        "debug_evaluate",
        json!({ "expression": "first_DebugLabel" }),
    );
    assert!(marked.starts_with(&value), "{marked} is not {value}");
    assert!(marked.contains("[first]"), "{marked}");
    let stale = server.succeed("debug_continue", json!({}));
    assert!(stale.starts_with("ended: exit status 0"), "{stale}");
}

/// The variable through which a debug build's server takes the wait of an
/// evaluation, in milliseconds.
#[cfg(debug_assertions)]
const EVALUATION_WAIT: &str = "NS_NEXT_MCP_EVALUATION_WAIT_MILLISECONDS";

/// Only a debug build reads the variable; a release build waits its 30 s.
#[cfg(debug_assertions)]
#[test]
fn an_evaluation_that_does_not_end_is_aborted() {
    let (_directory, path) = program("Counter", COUNTER);
    let mut server = McpServer::start_with_environment(&[(EVALUATION_WAIT, "300")]);
    server.succeed(
        "debug_start",
        json!({
            "program": path,
            "breakpoints": [{ "location": location(&path, 5) }],
        }),
    );
    let aborted = server.fail(
        "debug_evaluate",
        json!({ "expression": "[true] whileTrue: []" }),
    );
    assert!(aborted.contains("aborted"), "{aborted}");
    assert_eq!(
        server.succeed("debug_evaluate", json!({ "expression": "count" })),
        "41"
    );
    let ended = server.succeed("debug_continue", json!({}));
    assert_eq!(ended, "ended: exit status 0\noutput: \"count is\\n41\\n\"");
}

/// `square:` is sent a hundred times, so that the baseline JIT compiles it
/// at its second entry in a plain run.
const A_HOT_METHOD: &str = indoc! {"
    class Demo = Object new () (
      square: number = (
        ^number * number.
      )

      public main: platform args: arguments = (
        |
          total ::= 0.
        |
        1 to: 100 do: [:index | total:: total + (square: index)].
        platform transcript show: total printString; newLine.
      )
    )
"};

/// `trace` and `trace_filter` reach the program as `--trace` and one
/// `--trace-filter` each, and their lines arrive with its output. Without
/// a JIT nothing compiles and nothing is said, so the test answers nothing
/// there.
#[test]
fn a_trace_arrives_with_the_output() {
    let mut server = McpServer::start_plain();
    let (_directory, path) = program("Demo", A_HOT_METHOD);
    // A target without a JIT compiles no body and traces nothing: a plain
    // run says whether this one does.
    let mut direct = common::target_command(binary());
    direct.arg("run").arg(&path).arg("--trace=compile");
    direct.env_remove("NS_NEXT_NO_JIT");
    for mode in STRESS_MODES {
        direct.env_remove(format!("NS_NEXT_STRESS_{mode}"));
    }
    let direct = direct.output().expect("run the program");
    if !String::from_utf8_lossy(&direct.stderr).contains("[compile] ") {
        return;
    }
    let traced = server.succeed(
        "debug_start",
        json!({ "program": path, "trace": "compile" }),
    );
    assert!(
        started(&traced).1.starts_with("ended: exit status 0"),
        "{traced}"
    );
    assert!(traced.contains("[compile] "), "{traced}");
    assert!(
        traced.contains("[compile] {Demo>>square:} compiled"),
        "{traced}"
    );
    let filtered = server.succeed(
        "debug_start",
        json!({
            "program": path,
            "trace": "compile",
            "trace_filter": ["Demo>>nothing:", "Demo>>sq..."],
        }),
    );
    assert!(
        filtered.contains("[compile] {Demo>>square:} compiled"),
        "{filtered}"
    );
    let excluded = server.succeed(
        "debug_start",
        json!({
            "program": path,
            "trace": "compile",
            "trace_filter": ["Demo>>nothing:"],
        }),
    );
    assert!(!excluded.contains("{Demo>>square:}"), "{excluded}");
    let untraced = server.succeed("debug_start", json!({ "program": path }));
    assert!(!untraced.contains("[compile] "), "{untraced}");
}

/// The arguments of a start of `path`, a `COUNTER`, stopping at its line 5.
fn counter_start(path: &Path) -> Value {
    json!({
        "program": path,
        "breakpoints": [{ "location": location(path, 5) }],
    })
}

/// Every start opens a session of its own, numbered from 1, and a second
/// start leaves the first open.
#[test]
fn a_start_answers_its_session() {
    let (_directory, path) = program("Counter", COUNTER);
    let mut server = McpServer::start();
    let first = server.succeed("debug_start", counter_start(&path));
    let (number, summary) = started(&first);
    assert_eq!(number, 1, "{first}");
    assert!(summary.starts_with("stopped: breakpoint at "), "{first}");
    let second = server.succeed("debug_start", counter_start(&path));
    assert_eq!(started(&second).0, 2, "{second}");
    let stack = server.succeed("debug_stack", json!({ "session": 1 }));
    assert!(
        line_starting(&stack, "#0 ").contains("Counter>>main:args:"),
        "{stack}"
    );
}

/// With several sessions open a call names its own, and a refusal names
/// the open sessions with their programs and states; a stopped session is
/// closed, and the one left needs no number.
#[test]
fn several_sessions_need_their_number() {
    let (_directory, path) = program("Counter", COUNTER);
    let mut server = McpServer::start();
    server.succeed("debug_start", counter_start(&path));
    server.succeed("debug_start", counter_start(&path));
    assert_eq!(
        server.fail("debug_stack", json!({})),
        "2 sessions are open: session 1 (Counter.ns, stopped), \
         session 2 (Counter.ns, stopped) — pass session"
    );
    assert_eq!(
        server.fail("debug_stack", json!({ "session": 3 })),
        "no session 3 is open: session 1 (Counter.ns, stopped), \
         session 2 (Counter.ns, stopped)"
    );
    let stopped = server.succeed("debug_stop", json!({ "session": 1 }));
    assert!(stopped.starts_with("ended: "), "{stopped}");
    assert!(stopped.ends_with("\nsession 1 is closed"), "{stopped}");
    assert_eq!(
        server.fail("debug_stack", json!({ "session": 1 })),
        "no session 1 is open: session 2 (Counter.ns, stopped)"
    );
    let stack = server.succeed("debug_stack", json!({}));
    assert!(
        line_starting(&stack, "#0 ").contains("Counter>>main:args:"),
        "{stack}"
    );
    server.succeed("debug_stop", json!({}));
    assert_eq!(
        server.fail("debug_stack", json!({})),
        "no program is being debugged; debug_start starts one"
    );
    assert_eq!(
        server.fail("debug_stack", json!({ "session": 2 })),
        "no session 2 is open; debug_start starts one"
    );
}

/// A session waiting for its program holds neither another session's
/// calls, its start included, nor a `ping`. The wait is the longest a
/// tool takes and the program spins forever, so that nothing but the
/// test's own act ends the wait: each answer before it is told apart by
/// order alone, never by a clock.
#[test]
fn a_wait_holds_no_other_session() {
    let (_spin_directory, spin) = program("Spin", SPIN);
    let (_counter_directory, counter) = program("Counter", COUNTER);
    let mut server = McpServer::start();
    let running = server.succeed("debug_start", json!({ "program": spin, "wait": 0.5 }));
    assert!(started(&running).1.starts_with("running for "), "{running}");
    // Before the second start, whose process the server names last.
    let spinning = server.program_process(1);
    let wait = server.send_call("debug_wait", json!({ "session": 1, "seconds": 600 }));
    let answered = |server: &mut McpServer, identifier: i64, what: &str| {
        let response = server
            .response(identifier, TIMEOUT)
            .unwrap_or_else(|| panic!("{what} was not answered within {TIMEOUT:?}"));
        assert!(
            server.response(wait, Duration::ZERO).is_none(),
            "the wait answered before {what}"
        );
        response
    };
    let start = server.send_call("debug_start", counter_start(&counter));
    let (text, is_error) = text_of(&answered(&mut server, start, "the second start"));
    assert!(!is_error, "{text}");
    let (number, summary) = started(&text);
    assert_eq!(number, 2, "{text}");
    assert!(summary.starts_with("stopped: breakpoint at "), "{text}");
    let stack = server.send_call("debug_stack", json!({ "session": 2 }));
    let (text, is_error) = text_of(&answered(&mut server, stack, "the stack"));
    assert!(!is_error, "{text}");
    assert!(text.starts_with("#0 Counter>>main:args:"), "{text}");
    let ping = server.send("ping", json!({}));
    assert_eq!(answered(&mut server, ping, "the ping")["result"], json!({}));
    // The program's end ends the wait: its connection closes.
    kill_process(spinning);
    let waited = server
        .response(wait, TIMEOUT)
        .unwrap_or_else(|| panic!("the wait was not answered"));
    let (text, is_error) = text_of(&waited);
    assert!(!is_error, "{text}");
    assert!(text.starts_with("ended: "), "{text}");
    server.succeed("debug_stop", json!({ "session": 1 }));
}

/// A call queued behind a waiting one when the server's end comes is
/// refused, never served: the end cuts the wait, which answers where the
/// program is, and the call queued after it is answered that the server
/// is ending. The program's process ends with the server.
#[test]
fn a_call_queued_at_the_server_s_end_is_refused() {
    let (_directory, spin) = program("Spin", SPIN);
    let mut server = McpServer::start();
    let running = server.succeed("debug_start", json!({ "program": spin, "wait": 0.5 }));
    assert!(started(&running).1.starts_with("running for "), "{running}");
    let process = server.program_process(1);
    let wait = server.send_call("debug_wait", json!({ "seconds": 600 }));
    let queued = server.send_call("debug_wait", json!({ "seconds": 0 }));
    server.close_input();
    let (text, is_error) = text_of(
        &server
            .response(wait, TIMEOUT)
            .unwrap_or_else(|| panic!("the wait was not answered")),
    );
    assert!(!is_error, "{text}");
    assert!(text.starts_with("running for "), "{text}");
    let (text, is_error) = text_of(
        &server
            .response(queued, TIMEOUT)
            .unwrap_or_else(|| panic!("the queued call was not answered")),
    );
    assert!(is_error, "{text}");
    assert_eq!(text, "the server is ending");
    assert!(server.exited_within(TIMEOUT), "the server did not end");
    assert!(
        ended_within(process, TIMEOUT),
        "the program outlived its server"
    );
}

/// Closing the input ends the server and every session's program, the
/// one that waits included.
#[test]
fn closing_the_input_ends_every_session_s_program() {
    let (_spin_directory, spin) = program("Spin", SPIN);
    let (_counter_directory, counter) = program("Counter", COUNTER);
    let mut server = McpServer::start();
    server.succeed("debug_start", json!({ "program": spin, "wait": 0.5 }));
    let spinning = server.program_process(1);
    server.succeed("debug_start", counter_start(&counter));
    let stopped = server.program_process(2);
    assert_ne!(spinning, stopped, "both starts named one process");
    server.send_call("debug_wait", json!({ "session": 1, "seconds": 600 }));
    server.close_input();
    assert!(server.exited_within(TIMEOUT), "the server did not end");
    assert!(
        ended_within(spinning, TIMEOUT),
        "the waiting session's program outlived its server"
    );
    assert!(
        ended_within(stopped, TIMEOUT),
        "the stopped session's program outlived its server"
    );
}

/// Starts `SPIN` and answers its running start's text: the program
/// spins forever, so that nothing but the test's own act ends a wait.
fn spinning_start(server: &mut McpServer, spin: &Path) -> String {
    let running = server.succeed("debug_start", json!({ "program": spin, "wait": 0.5 }));
    assert!(started(&running).1.starts_with("running for "), "{running}");
    running
}

/// A `debug_pause` made while a `debug_continue` waits pauses the program
/// at once, and both answer the pause. Only the pause can end the wait,
/// which asks for the longest a tool waits.
#[test]
fn a_pause_reaches_a_waiting_continue() {
    let (_directory, spin) = program("Spin", SPIN);
    let mut server = McpServer::start_plain();
    let entry = server.succeed(
        "debug_start",
        json!({ "program": spin, "stop_on_entry": true }),
    );
    assert!(
        started(&entry).1.starts_with("stopped: entry at "),
        "{entry}"
    );
    let waiting = server.send_call("debug_continue", json!({ "wait": 600 }));
    let pause = server.send_call("debug_pause", json!({}));
    let (paused, is_error) = server.answered(pause, "the pause");
    assert!(!is_error, "{paused}");
    assert!(paused.starts_with("stopped: pause at "), "{paused}");
    let (continued, is_error) = server.answered(waiting, "the continue");
    assert!(!is_error, "{continued}");
    assert!(continued.starts_with("stopped: pause at "), "{continued}");
}

/// A `debug_stop` made while a `debug_wait` waits ends the program at
/// once and closes the session; the wait answers the end.
#[test]
fn a_stop_reaches_a_waiting_wait() {
    let (_directory, spin) = program("Spin", SPIN);
    let mut server = McpServer::start_plain();
    spinning_start(&mut server, &spin);
    let waiting = server.send_call("debug_wait", json!({ "seconds": 600 }));
    let stop = server.send_call("debug_stop", json!({}));
    let (stopped, is_error) = server.answered(stop, "the stop");
    assert!(!is_error, "{stopped}");
    assert!(stopped.starts_with("ended: "), "{stopped}");
    assert!(stopped.ends_with("\nsession 1 is closed"), "{stopped}");
    let (waited, is_error) = server.answered(waiting, "the wait");
    assert!(!is_error, "{waited}");
    assert!(waited.starts_with("ended: "), "{waited}");
}

/// A cancelled wait ends at once and answers nothing: the pause made
/// after it finds no wait to act on and answers alone, and the wait's
/// answer, which would come before the pause's, never comes.
#[test]
fn a_cancelled_wait_answers_nothing() {
    let (_directory, spin) = program("Spin", SPIN);
    let mut server = McpServer::start_plain();
    spinning_start(&mut server, &spin);
    let waiting = server.send_call("debug_wait", json!({ "seconds": 600 }));
    server.cancel(waiting);
    let pause = server.send_call("debug_pause", json!({}));
    let (paused, is_error) = server.answered(pause, "the pause");
    assert!(!is_error, "{paused}");
    assert!(paused.starts_with("stopped: pause at "), "{paused}");
    assert!(
        server.response(waiting, Duration::from_secs(1)).is_none(),
        "the cancelled wait was answered"
    );
}

/// A cancelled call still queued behind a waiting one is dropped, never
/// served: the call made after it answers, and the cancelled one, which
/// would have answered before it, does not.
#[test]
fn a_cancelled_queued_call_is_dropped() {
    let (_directory, spin) = program("Spin", SPIN);
    let mut server = McpServer::start_plain();
    spinning_start(&mut server, &spin);
    let waiting = server.send_call("debug_wait", json!({ "seconds": 600 }));
    let queued = server.send_call("debug_wait", json!({ "seconds": 0 }));
    server.cancel(queued);
    let after = server.send_call("debug_wait", json!({ "seconds": 0 }));
    let pause = server.send_call("debug_pause", json!({}));
    let (paused, is_error) = server.answered(pause, "the pause");
    assert!(!is_error, "{paused}");
    let (waited, is_error) = server.answered(waiting, "the wait");
    assert!(
        !is_error && waited.starts_with("stopped: pause at "),
        "{waited}"
    );
    let (text, is_error) = server.answered(after, "the call after the cancelled one");
    assert!(
        !is_error && text.starts_with("stopped: pause at "),
        "{text}"
    );
    assert!(
        server.response(queued, Duration::ZERO).is_none(),
        "the cancelled call was answered"
    );
}

/// A cancelled evaluation is aborted as its timeout aborts it: the
/// evaluation made after it answers, the program still stopped. Its own
/// wait is far longer than the test's bound, so that only the abort lets
/// the second evaluation run.
#[cfg(debug_assertions)]
#[test]
fn a_cancelled_evaluation_is_aborted() {
    let (_directory, path) = program("Counter", COUNTER);
    let mut server = McpServer::launch(Path::new(binary()), &[(EVALUATION_WAIT, "600000")], true);
    server.succeed("debug_start", counter_start(&path));
    let endless = server.send_call(
        "debug_evaluate",
        json!({ "expression": "[true] whileTrue: []" }),
    );
    server.cancel(endless);
    let count = server.send_call("debug_evaluate", json!({ "expression": "count" }));
    assert_eq!(
        server.answered(count, "the evaluation after the cancelled one"),
        ("41".to_string(), false)
    );
    assert!(
        server.response(endless, Duration::ZERO).is_none(),
        "the cancelled evaluation was answered"
    );
}

/// A `debug_stop` made while an evaluation runs aborts it and ends the
/// program at once, without waiting out the evaluation's own wait.
#[cfg(debug_assertions)]
#[test]
fn a_stop_aborts_a_running_evaluation() {
    let (_directory, path) = program("Counter", COUNTER);
    let mut server = McpServer::launch(Path::new(binary()), &[(EVALUATION_WAIT, "600000")], true);
    server.succeed("debug_start", counter_start(&path));
    let endless = server.send_call(
        "debug_evaluate",
        json!({ "expression": "[true] whileTrue: []" }),
    );
    let stop = server.send_call("debug_stop", json!({}));
    let (aborted, is_error) = server.answered(endless, "the evaluation");
    assert!(is_error, "{aborted}");
    assert!(aborted.contains("aborted"), "{aborted}");
    let (stopped, is_error) = server.answered(stop, "the stop");
    assert!(!is_error, "{stopped}");
    assert!(stopped.starts_with("ended: "), "{stopped}");
    assert!(stopped.ends_with("\nsession 1 is closed"), "{stopped}");
}

/// Closing the input while a wait runs ends the wait, the server and the
/// program's process.
#[test]
fn closing_the_input_ends_a_waiting_server() {
    let (_directory, spin) = program("Spin", SPIN);
    let mut server = McpServer::start_plain();
    spinning_start(&mut server, &spin);
    let process = server.program_process(1);
    server.send_call("debug_wait", json!({ "seconds": 600 }));
    server.close_input();
    assert!(server.exited_within(TIMEOUT), "the server did not end");
    assert!(
        ended_within(process, TIMEOUT),
        "the program outlived its server"
    );
}

/// Whether the process `process` ends within `within`.
fn ended_within(process: u32, within: Duration) -> bool {
    let started = Instant::now();
    while process_runs(process) {
        if started.elapsed() > within {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

/// Kills the process `process`.
fn kill_process(process: u32) {
    let status = if cfg!(windows) {
        std::process::Command::new("taskkill")
            .args(["/PID", &process.to_string(), "/F"])
            .stdout(Stdio::null())
            .status()
    } else {
        std::process::Command::new("kill")
            .args(["-9", &process.to_string()])
            .status()
    };
    assert!(
        status.is_ok_and(|status| status.success()),
        "kill process {process}"
    );
}

/// A batch is answered as one array, in its members' order, once each
/// member is answered, a tool call's from its session's worker; its
/// notification is not answered.
#[test]
fn a_batch_is_answered_as_one_array_in_its_order() {
    let (_directory, path) = program("Counter", COUNTER);
    let mut server = McpServer::start();
    server.write(&json!([
        {
            "jsonrpc": "2.0", "id": 101, "method": "tools/call",
            "params": { "name": "debug_start", "arguments": counter_start(&path) }
        },
        { "jsonrpc": "2.0", "method": "notifications/initialized" },
        { "jsonrpc": "2.0", "id": 102, "method": "ping" },
        {
            "jsonrpc": "2.0", "id": 103, "method": "tools/call",
            "params": { "name": "debug_stack", "arguments": { "session": 7 } }
        }
    ]));
    let answer = server.next_message();
    let members = answer.as_array().unwrap_or_else(|| panic!("{answer}"));
    let identifiers: Vec<&Value> = members.iter().map(|member| &member["id"]).collect();
    assert_eq!(
        identifiers,
        [&json!(101), &json!(102), &json!(103)],
        "{answer}"
    );
    let (text, is_error) = text_of(&members[0]);
    assert!(!is_error && started(&text).0 == 1, "{text}");
    assert_eq!(members[1]["result"], json!({}), "{answer}");
    let (text, is_error) = text_of(&members[2]);
    assert!(is_error, "{text}");
    assert!(
        text.starts_with("no session 7 is open: session 1 ("),
        "{text}"
    );
}

/// A batch's cancelled member is left out of the batch's answer, which
/// answers every other member.
#[test]
fn a_cancelled_member_of_a_batch_is_left_out() {
    let (_directory, spin) = program("Spin", SPIN);
    let mut server = McpServer::start_plain();
    spinning_start(&mut server, &spin);
    server.write(&json!([
        {
            "jsonrpc": "2.0", "id": 201, "method": "tools/call",
            "params": { "name": "debug_wait", "arguments": { "seconds": 600 } }
        },
        { "jsonrpc": "2.0", "id": 202, "method": "ping" }
    ]));
    server.cancel(201);
    let answer = server.next_message();
    assert_eq!(
        answer,
        json!([{ "jsonrpc": "2.0", "id": 202, "result": {} }]),
        "{answer}"
    );
}

/// Calls `debug_wait` without waiting until the output the answers since
/// `first` showed holds `text`; fails past `TIMEOUT`. The program's output
/// reaches the server through a pipe, so a later answer may be the one to
/// show it; an output an answer lost is never shown.
fn shown_eventually(server: &mut McpServer, first: &str, text: &str) {
    let started = Instant::now();
    let mut shown = first.to_string();
    while !shown.contains(text) {
        assert!(
            started.elapsed() < TIMEOUT,
            "no answer showed {text:?}: {shown}"
        );
        shown.push_str(&server.succeed("debug_wait", json!({ "seconds": 0 })));
    }
}

/// An answer the cancelled set drops gives its output to the next answer,
/// also when the call never heard of the cancellation: here the start was
/// cancelled before it was made, when no session could hear of it, so
/// that only the set knows. The program prints `start` before it stops at
/// its breakpoint.
#[test]
fn output_a_dropped_answer_took_is_shown_by_the_next() {
    let (_directory, path) = program("Counter", COUNTER);
    let mut server = McpServer::start_plain();
    let next = server.next_id;
    server.cancel(next);
    let start = server.send_call("debug_start", counter_start(&path));
    assert_eq!(start, next);
    let stopped = server.succeed("debug_wait", json!({ "seconds": 0 }));
    assert!(stopped.starts_with("stopped: breakpoint at "), "{stopped}");
    assert!(
        server.response(start, Duration::ZERO).is_none(),
        "the cancelled start was answered"
    );
    shown_eventually(&mut server, &stopped, "output: \"start\\n");
}

/// A cancelled start ends its wait at once and answers nothing, but keeps
/// its session, whose next answers show the output the start took.
#[test]
fn a_cancelled_start_keeps_its_session() {
    let (_directory, spin) = program("Spin", SPIN);
    let mut server = McpServer::start_plain();
    let start = server.send_call("debug_start", json!({ "program": spin, "wait": 600 }));
    server.cancel(start);
    let waited = server.succeed("debug_wait", json!({ "seconds": 0 }));
    assert!(waited.starts_with("running for "), "{waited}");
    assert!(
        server.response(start, Duration::ZERO).is_none(),
        "the cancelled start was answered"
    );
    shown_eventually(&mut server, &waited, "output: \"spinning\\n");
}

/// A `debug_pause` made while a start waits for its program's first stop
/// pauses the program, and both answer the pause.
#[test]
fn a_pause_reaches_a_waiting_start() {
    let (_directory, spin) = program("Spin", SPIN);
    let mut server = McpServer::start_plain();
    let start = server.send_call("debug_start", json!({ "program": spin, "wait": 600 }));
    let pause = server.send_call("debug_pause", json!({}));
    let (paused, is_error) = server.answered(pause, "the pause");
    assert!(
        !is_error && paused.starts_with("stopped: pause at "),
        "{paused}"
    );
    let (text, is_error) = server.answered(start, "the start");
    assert!(!is_error, "{text}");
    assert!(started(&text).1.starts_with("stopped: pause at "), "{text}");
}

/// A `debug_stop` made while a start waits ends the program and closes the
/// session; the start answers the end.
#[test]
fn a_stop_reaches_a_waiting_start() {
    let (_directory, spin) = program("Spin", SPIN);
    let mut server = McpServer::start_plain();
    let start = server.send_call("debug_start", json!({ "program": spin, "wait": 600 }));
    let stop = server.send_call("debug_stop", json!({}));
    let (stopped, is_error) = server.answered(stop, "the stop");
    assert!(!is_error, "{stopped}");
    assert!(stopped.ends_with("\nsession 1 is closed"), "{stopped}");
    let (text, is_error) = server.answered(start, "the start");
    assert!(!is_error, "{text}");
    assert!(started(&text).1.starts_with("ended: "), "{text}");
}
