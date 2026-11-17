//! `ns-next mcp`, the Model Context Protocol server through which an agent
//! debugs a program (`docs/design/debugger.md`, "The MCP server"): each
//! test starts the server as a child and speaks JSON-RPC to it, one
//! message a line, as an MCP client does.

mod common;

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

/// The tools and their parameters, the required ones first.
const TOOLS: [(&str, &[&str], &[&str]); 16] = [
    (
        "debug_start",
        &["program"],
        &[
            "breakpoints",
            "stop_on_entry",
            "arguments",
            "runtime",
            "wait",
        ],
    ),
    ("debug_breakpoints", &[], &["set", "clear"]),
    ("debug_continue", &[], &["wait"]),
    ("debug_step", &["kind"], &["target", "wait"]),
    ("debug_step_targets", &[], &[]),
    ("debug_run_to", &["location"], &["wait"]),
    ("debug_pause", &[], &[]),
    ("debug_wait", &["seconds"], &[]),
    ("debug_stack", &[], &[]),
    (
        "debug_variables",
        &[],
        &["frame", "reference", "start", "count"],
    ),
    ("debug_evaluate", &["expression"], &["frame"]),
    ("debug_set_variable", &["name", "value"], &["frame"]),
    ("debug_frame", &["action", "frame"], &["value"]),
    ("debug_memory", &[], &["class", "referrers_of"]),
    ("debug_mark", &["label"], &["reference"]),
    ("debug_stop", &[], &[]),
];

/// A running `ns-next mcp` and the lines it writes to its standard output.
/// Its input is closed once the process started ends, as Node's
/// `child_process` closes it, on which the MCP SDKs' stdio transports are
/// built: a server that handed over to another process would lose its
/// input then.
struct McpServer {
    /// The directory of the copy the server was started from, if any.
    build: Option<ScratchDirectory>,
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    lines: Receiver<String>,
    stderr: Arc<Mutex<Vec<u8>>>,
    next_id: i64,
}

impl McpServer {
    /// `ns-next mcp` started from a copy of the binary of its own, since
    /// the server renames the file it was started from on Windows,
    /// initialized.
    fn start() -> McpServer {
        McpServer::start_from_a_copy(&[])
    }

    /// As `start`, with the environment `variables`.
    fn start_from_a_copy(variables: &[(&str, &str)]) -> McpServer {
        let build = ScratchDirectory::new("server");
        let name = Path::new(binary()).file_name().expect("a file name");
        let executable = build.join(&name.to_string_lossy());
        std::fs::copy(binary(), &executable).expect("copy the binary");
        let mut server = McpServer::start_with(&executable, variables);
        server.build = Some(build);
        server
    }

    /// `<executable> mcp` with the environment `variables`, initialized.
    fn start_with(executable: &Path, variables: &[(&str, &str)]) -> McpServer {
        let mut command = common::target_command(executable);
        command
            .arg("mcp")
            .envs(variables.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
            build: None,
            child,
            stdin,
            lines,
            stderr,
            next_id: 1,
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

    /// Whether the process started still runs.
    fn runs(&self) -> bool {
        self.child
            .lock()
            .expect("the child")
            .try_wait()
            .is_ok_and(|status| status.is_none())
    }

    /// The process id of the program the last `debug_start` started, as
    /// the server says it on its standard error.
    fn program_process(&self) -> u32 {
        let stderr = String::from_utf8_lossy(&self.stderr.lock().expect("stderr")).to_string();
        let line = stderr
            .lines()
            .rev()
            .find_map(|line| line.split(" runs as process ").nth(1))
            .unwrap_or_else(|| panic!("no program's process on stderr: {stderr}"));
        line.trim().parse().expect("a process id")
    }

    fn notify(&mut self, method: &str) {
        self.write(&json!({ "jsonrpc": "2.0", "method": method }));
    }

    /// Sends a request and answers its response; every line the server
    /// writes must be a JSON-RPC message.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let line = self.lines.recv_timeout(TIMEOUT).unwrap_or_else(|_| {
                panic!(
                    "no answer to `{method}` within {TIMEOUT:?}; stderr: {}",
                    String::from_utf8_lossy(&self.stderr.lock().expect("stderr"))
                )
            });
            let message: Value = serde_json::from_str(&line)
                .unwrap_or_else(|error| panic!("not a JSON-RPC message ({error}): {line}"));
            assert_eq!(message["jsonrpc"], "2.0", "{message}");
            if message["id"] == id {
                return message;
            }
        }
    }

    /// Calls `tool`; answers its text and whether it is an error.
    fn call(&mut self, tool: &str, arguments: Value) -> (String, bool) {
        let response = self.request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        );
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
    let first = answer.lines().next().expect("a first line");
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
    assert!(entry.starts_with("stopped: entry at "), "{entry}");
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
            "breakpoints": [{ "location": "Kernel.Array>>do:" }],
        }),
    );
    let first = answer.lines().next().expect("a first line");
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

/// The server stays the process its client started, and frees the file
/// it was started from for the next build: a build overwrites it while
/// the server and a session run, and the next `debug_start` runs the new
/// file. On Windows the server renames its running file aside and puts a
/// copy back, after removing what earlier servers renamed aside that no
/// process runs any more.
#[test]
fn the_server_frees_its_executable_for_the_next_build() {
    let build = ScratchDirectory::new("build");
    let name = Path::new(binary())
        .file_name()
        .expect("a file name")
        .to_string_lossy()
        .to_string();
    let executable = std::path::absolute(build.join(&name)).expect("an absolute path");
    std::fs::copy(binary(), &executable).expect("copy the binary");
    let leftover = build.join(&format!("{name}.4294967295.running"));
    std::fs::write(&leftover, "an earlier server's").expect("write a leftover");
    let mut server = McpServer::start_with(&executable, &[]);
    assert!(server.runs(), "the process started ended");
    if cfg!(windows) {
        assert!(!leftover.exists(), "the leftover is still there");
        let aside = build.join(&format!(
            "{name}.{}.running",
            server.child.lock().expect("the child").id()
        ));
        assert!(aside.is_file(), "nothing renamed aside");
    }
    // A build overwrites the executable while the server runs.
    replace_as_a_build_does(&executable);
    let (_directory, path) = program("Counter", COUNTER);
    let ended = server.succeed("debug_start", json!({ "program": path }));
    assert!(ended.starts_with("ended: exit status 0"), "{ended}");
    assert!(
        line_starting(&ended, "runtime: ").contains(&executable.display().to_string()),
        "{ended}"
    );
    // Nor does a session hold the executable it runs.
    server.succeed(
        "debug_start",
        json!({ "program": path, "stop_on_entry": true }),
    );
    replace_as_a_build_does(&executable);
    assert!(server.runs(), "the process started ended");
}

/// Every file below `directory` whose name is `name`, at any depth.
fn files_named(directory: &Path, name: &OsStr) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name() == Some(name) {
                found.push(path);
            }
        }
    }
    found
}

/// A session runs the binary it names from a copy only on Windows, where
/// a running file cannot be replaced, and that copy lies in the user's
/// own local data directory, never in the shared temporary one, where
/// another user could have put a file of their own first: a copy found
/// there whose bytes are not the binary's is written again before it
/// runs. Elsewhere nothing is copied.
#[test]
fn a_session_runs_a_verified_copy_in_the_user_s_own_directory() {
    let data = ScratchDirectory::new("local-data");
    let temporary = ScratchDirectory::new("temporary");
    let data_path = data.path().display().to_string();
    let temporary_path = temporary.path().display().to_string();
    let mut server = McpServer::start_from_a_copy(&[
        ("LOCALAPPDATA", data_path.as_str()),
        ("XDG_CACHE_HOME", data_path.as_str()),
        ("TEMP", temporary_path.as_str()),
        ("TMP", temporary_path.as_str()),
        ("TMPDIR", temporary_path.as_str()),
    ]);
    let (_directory, path) = program("Counter", COUNTER);
    let ended = server.succeed("debug_start", json!({ "program": path }));
    assert!(ended.starts_with("ended: exit status 0"), "{ended}");
    let name = Path::new(binary()).file_name().expect("a file name");
    let mut copies = files_named(data.path(), name);
    copies.extend(files_named(temporary.path(), name));
    if cfg!(windows) {
        let [copy] = copies.as_slice() else {
            panic!("one copy was expected, not {copies:?}");
        };
        std::fs::write(copy, "not the binary").expect("replace the copy");
        let ended = server.succeed("debug_start", json!({ "program": path }));
        assert!(ended.starts_with("ended: exit status 0"), "{ended}");
        let written = std::fs::read(copy).expect("read the copy");
        let original = std::fs::read(binary()).expect("read the binary");
        assert!(written == original, "the copy was not written again");
        assert!(
            copy.starts_with(data.path()),
            "the copy lies outside the user's own directory: {}",
            copy.display()
        );
    } else {
        assert!(copies.is_empty(), "the binary was copied: {copies:?}");
    }
    assert!(
        std::fs::read_dir(temporary.path()).is_ok_and(|mut entries| entries.next().is_none()),
        "the shared temporary directory holds the server's files"
    );
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
    let server = McpServer::start_from_a_copy(&[
        ("LOCALAPPDATA", data_path.as_str()),
        ("XDG_CACHE_HOME", data_path.as_str()),
    ]);
    assert!(!stale.exists(), "the stale sources are still there");
    assert!(own.is_dir(), "the binary's own sources were removed");
    assert!(fresh.is_dir(), "sources touched since were removed");
    drop(server);
}

/// Puts a new file at `executable`'s path as a build does: written beside
/// it and moved into place, which replaces a file a process runs on every
/// system but Windows, and on Windows one that no process runs.
fn replace_as_a_build_does(executable: &Path) {
    let mut written = executable.as_os_str().to_os_string();
    written.push(".written");
    let written = PathBuf::from(written);
    std::fs::copy(binary(), &written).expect("write the new build");
    std::fs::rename(&written, executable).expect("move the new build into place");
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
    assert!(running.starts_with("running for "), "{running}");
    let process = server.program_process();
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
const EVALUATION_WAIT: &str = "NS_NEXT_MCP_EVALUATION_WAIT_MILLISECONDS";

/// Only a debug build reads the variable; a release build waits its 30 s.
#[cfg(debug_assertions)]
#[test]
fn an_evaluation_that_does_not_end_is_aborted() {
    let (_directory, path) = program("Counter", COUNTER);
    let mut server = McpServer::start_from_a_copy(&[(EVALUATION_WAIT, "300")]);
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
