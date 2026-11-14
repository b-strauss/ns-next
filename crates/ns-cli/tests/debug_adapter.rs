//! `ns-next run --debug` and `--debug-connect`: the agent's connection,
//! its session and its link to the host's runs, through a DAP client
//! (`common/dap.rs`) against a child `ns-next`.

mod common;
#[path = "common/dap.rs"]
mod dap;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::ScratchDirectory;
use dap::DapClient;
use indoc::indoc;
use serde_json::{json, Value};

/// The line a terminated program's host send reports.
const TERMINATED: &str = "Uncaught Newspeak error: the debugger terminated the program";

/// A source file's root whose `main:args:` prints `hi`.
const HELLO: &str = indoc! {"
    class Hello = Object new () (
      public main: platform args: arguments = (
        platform transcript show: 'hi'; newLine.
      )
    ) : ()
"};

/// A program that says it loops, then loops forever inside an `ensure:`.
const LOOPER: &str = indoc! {"
    class Looper = Object new () (
      public main: platform args: arguments = (
        | out = platform transcript. |
        out show: 'looping'; newLine.
        [[true] whileTrue: []] ensure: [out show: 'ensured'; newLine].
      )
    ) : ()
"};

/// A project's root whose initializer runs a while before `main:args:`.
const SLOW_START: &str = indoc! {"
    class SlowStart packageUsing: manifest = Object new (
      |
        count ::= 0.
      |
      1 to: 200000 do: [:index | count:: count + 1].
    ) (
      public main: platform args: arguments = (
        platform transcript show: 'main ran'; newLine.
      )
    ) : ()
"};

/// A program that says it computes, then spends long in one leaf.
const LONG_LEAF: &str = indoc! {"
    class LongLeaf = Object new () (
      public main: platform args: arguments = (
        platform transcript show: 'computing'; newLine.
        (10 raisedTo: 2000000) printString.
        platform transcript show: 'done'; newLine.
      )
    ) : ()
"};

/// A scratch directory holding `source` as `<name>.ns`, and the file's path.
fn program(name: &str, source: &str) -> (ScratchDirectory, PathBuf) {
    let directory = ScratchDirectory::new(name);
    let path = directory.join(&format!("{name}.ns"));
    std::fs::write(&path, source).expect("write the program");
    (directory, path)
}

fn names(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| event["event"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `exited` event's exit code, asserting `terminated` follows it.
fn exit_code_reported(client: &mut DapClient) -> i64 {
    let events = client.connection.events_until_closed();
    let names = names(&events);
    let exited = names
        .iter()
        .position(|name| name == "exited")
        .unwrap_or_else(|| panic!("no `exited` event: {names:?}"));
    assert_eq!(
        names.get(exited + 1).map(String::as_str),
        Some("terminated"),
        "{names:?}"
    );
    events[exited]["body"]["exitCode"]
        .as_i64()
        .expect("an exit code")
}

#[test]
fn a_session_runs_the_program_to_its_end() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    let process = client.attach(false);
    assert_eq!(
        process["body"]["systemProcessId"],
        json!(client.process_id()),
        "{process}"
    );
    assert_eq!(process["body"]["startMethod"], "attach", "{process}");
    client.configuration_done();
    assert_eq!(exit_code_reported(&mut client), 0);
    let finished = client.finish();
    assert_eq!(finished.status, Some(0), "{}", finished.stderr);
    assert_eq!(finished.stdout, "hi\n");
    assert_eq!(finished.stderr, "");
}

#[test]
fn the_listening_mode_prints_its_port_and_token_and_requires_it() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::listen_mode(&path);
    client.initialize();
    let wrong = "0".repeat(32);
    assert_ne!(client.token(), wrong);
    let refused = client.request("attach", json!({ "token": wrong }));
    assert_eq!(refused["success"], false, "{refused}");
    client.connection.expect_closed();

    client.reconnect();
    client.start_session(false);
    assert_eq!(exit_code_reported(&mut client), 0);
    let finished = client.finish();
    assert_eq!(finished.status, Some(0), "{}", finished.stderr);
    assert_eq!(finished.stdout, "hi\n");
    let lines: Vec<&str> = finished.stderr.lines().collect();
    assert_eq!(lines.len(), 1, "{}", finished.stderr);
    dap::parse_listening_line(lines[0]);
}

#[test]
fn stop_on_entry_stops_at_main() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(true);
    let stopped = client.event("stopped");
    assert_eq!(stopped["body"]["reason"], "entry", "{stopped}");
    let trace = client.request("stackTrace", json!({ "threadId": 1 }));
    assert_eq!(trace["success"], true, "{trace}");
    let frames = trace["body"]["stackFrames"]
        .as_array()
        .expect("the stack's frames");
    assert_eq!(frames.len(), 1, "{trace}");
    assert_eq!(frames[0]["name"], "Hello>>main:args:", "{trace}");
    let continued = client.request("continue", json!({ "threadId": 1 }));
    assert_eq!(continued["success"], true, "{continued}");
    assert_eq!(exit_code_reported(&mut client), 0);
    let finished = client.finish();
    assert_eq!(finished.status, Some(0), "{}", finished.stderr);
    assert_eq!(finished.stdout, "hi\n");
}

#[test]
fn terminate_runs_the_ensure_blocks() {
    let (_directory, path) = program("Looper", LOOPER);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(false);
    client.wait_for_stdout("looping\n");
    let paused = client.request("pause", json!({ "threadId": 1 }));
    assert_eq!(paused["success"], true, "{paused}");
    let stopped = client.event("stopped");
    assert_eq!(stopped["body"]["reason"], "pause", "{stopped}");
    let terminated = client.request("terminate", json!({}));
    assert_eq!(terminated["success"], true, "{terminated}");
    assert_eq!(exit_code_reported(&mut client), 1);
    let finished = client.finish();
    assert_eq!(finished.status, Some(1), "{}", finished.stderr);
    assert!(
        finished.stdout.ends_with("ensured\n"),
        "{}",
        finished.stdout
    );
    assert!(finished.stderr.contains(TERMINATED), "{}", finished.stderr);
}

#[test]
fn a_terminate_between_runs_ends_the_program_before_main() {
    let directory = ScratchDirectory::new("SlowStart");
    std::fs::write(directory.join("SlowStart.ns"), SLOW_START).expect("write the root");
    std::fs::write(
        directory.join("newspeak.json"),
        r#"{ "main": "SlowStart.ns" }"#,
    )
    .expect("write the project file");
    let mut client = DapClient::connect_mode(directory.path());
    client.start_session(false);
    let terminated = client.request("terminate", json!({}));
    assert_eq!(terminated["success"], true, "{terminated}");
    assert_eq!(exit_code_reported(&mut client), 1);
    let finished = client.finish();
    assert_eq!(finished.status, Some(1), "{}", finished.stderr);
    assert!(!finished.stdout.contains("main ran"), "{}", finished.stdout);
    assert!(
        finished
            .stderr
            .contains("the debugger terminated the program"),
        "{}",
        finished.stderr
    );
}

#[test]
fn disconnect_with_terminate_debuggee_kills_a_program_in_a_long_leaf() {
    let (_directory, path) = program("LongLeaf", LONG_LEAF);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(false);
    client.wait_for_stdout("computing\n");
    client
        .connection
        .send("disconnect", json!({ "terminateDebuggee": true }));
    let finished = client.finish_within(Duration::from_secs(5));
    assert_eq!(finished.status, Some(1), "{}", finished.stderr);
    assert!(!finished.stdout.contains("done"), "{}", finished.stdout);
    assert!(finished.stderr.contains(TERMINATED), "{}", finished.stderr);
}

fn disconnect_while_stopped(mut client: DapClient) {
    client.start_session(true);
    let stopped = client.event("stopped");
    assert_eq!(stopped["body"]["reason"], "entry", "{stopped}");
    let disconnected = client.request("disconnect", json!({}));
    assert_eq!(disconnected["success"], true, "{disconnected}");
    let finished = client.finish();
    assert_eq!(finished.status, Some(0), "{}", finished.stderr);
    assert_eq!(finished.stdout, "hi\n");
}

#[test]
fn disconnecting_while_stopped_lets_the_program_finish_in_the_connecting_mode() {
    let (_directory, path) = program("Hello", HELLO);
    disconnect_while_stopped(DapClient::connect_mode(&path));
}

#[test]
fn disconnecting_while_stopped_lets_the_program_finish_in_the_listening_mode() {
    let (_directory, path) = program("Hello", HELLO);
    disconnect_while_stopped(DapClient::listen_mode(&path));
}

#[test]
fn a_second_client_connects_after_the_first_disconnected() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::listen_mode(&path);
    client.initialize();
    client.attach(false);
    let disconnected = client.request("disconnect", json!({}));
    assert_eq!(disconnected["success"], true, "{disconnected}");
    client.connection.expect_closed();

    client.reconnect();
    client.start_session(false);
    assert_eq!(exit_code_reported(&mut client), 0);
    let finished = client.finish();
    assert_eq!(finished.status, Some(0), "{}", finished.stderr);
    assert_eq!(finished.stdout, "hi\n");
}

#[test]
fn a_client_that_cannot_be_reached_is_reported() {
    let (_directory, path) = program("Hello", HELLO);
    // Port 0, which nothing listens on and no process can take, and which
    // every system refuses at once, where a closed port costs Windows
    // seconds of retries.
    let address = "127.0.0.1:0";
    let (success, stdout, stderr) = common::ns_next(&[
        "run".as_ref(),
        format!("--debug-connect={address}").as_ref(),
        path.as_os_str(),
    ]);
    assert!(!success);
    assert_eq!(stdout, "");
    assert!(
        stderr.starts_with(&format!("error: connecting the debugger to {address}: ")),
        "{stderr}"
    );
}

#[test]
fn an_initialize_counting_from_0_is_refused() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::connect_mode(&path);
    for (lines, columns) in [(false, true), (true, false)] {
        let refused = client.request(
            "initialize",
            json!({
                "adapterID": "ns-next",
                "linesStartAt1": lines,
                "columnsStartAt1": columns,
            }),
        );
        assert_eq!(refused["success"], false, "{refused}");
    }
    client.start_session(false);
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().status, Some(0));
}

/// An `attach` without a token, and one whose arguments are no attach's,
/// are refused and their connections closed; the program waits for a
/// client that presents the token.
#[test]
fn an_attach_without_the_token_is_refused_and_closed() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::listen_mode(&path);
    for arguments in [json!({}), json!({ "token": 123 })] {
        client.initialize();
        let refused = client.request("attach", arguments);
        assert_eq!(refused["success"], false, "{refused}");
        client.connection.expect_closed();
        client.reconnect();
    }
    client.start_session(false);
    assert_eq!(exit_code_reported(&mut client), 0);
    let finished = client.finish();
    assert_eq!(finished.status, Some(0), "{}", finished.stderr);
    assert_eq!(finished.stdout, "hi\n");
}

/// Before an `attach` with the token, a listening agent refuses every
/// request but `initialize` -- a `disconnect` that would end the process
/// included -- and closes the connection; the program lives on for the
/// next client.
#[test]
fn a_request_before_the_attach_is_refused_without_ending_the_program() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::listen_mode(&path);
    client.initialize();
    let refused = client.request("disconnect", json!({ "terminateDebuggee": true }));
    assert_eq!(refused["success"], false, "{refused}");
    client.connection.expect_closed();

    client.reconnect();
    client.start_session(false);
    assert_eq!(exit_code_reported(&mut client), 0);
    let finished = client.finish();
    assert_eq!(finished.status, Some(0), "{}", finished.stderr);
    assert_eq!(finished.stdout, "hi\n");
}

/// A `pause` sent right after a `continue` is asked of the running
/// program, and stops it.
#[test]
fn a_pause_right_after_a_continue_stops_the_program() {
    let (_directory, path) = program("Looper", LOOPER);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(true);
    let stopped = client.event("stopped");
    assert_eq!(stopped["body"]["reason"], "entry", "{stopped}");
    let continue_seq = client.connection.send("continue", json!({ "threadId": 1 }));
    let pause_seq = client.connection.send("pause", json!({ "threadId": 1 }));
    let continued = client.connection.response_to(continue_seq, "continue");
    assert_eq!(continued["success"], true, "{continued}");
    let paused = client.connection.response_to(pause_seq, "pause");
    assert_eq!(paused["success"], true, "{paused}");
    let stopped = client.event("stopped");
    assert_eq!(stopped["body"]["reason"], "pause", "{stopped}");
    let terminated = client.request("terminate", json!({}));
    assert_eq!(terminated["success"], true, "{terminated}");
    assert_eq!(exit_code_reported(&mut client), 1);
    assert_eq!(client.finish().status, Some(1));
}

/// The variable a debug build of `ns-next` reads its deadline to attach
/// from, in milliseconds; a release build keeps its 10 s. The tests that
/// shorten it run only in a debug build: this binary and `ns-next` are
/// built in one profile, and without the override the deadline would
/// outlast the trickle test and leave the silence test proving nothing.
#[cfg(debug_assertions)]
const ATTACH_DEADLINE: (&str, &str) = ("NS_NEXT_DEBUG_ATTACH_DEADLINE_MILLISECONDS", "200");

/// The deadline to attach ends nothing once the client attached: a client
/// silent for longer than it after its `attach` keeps its session.
#[cfg(debug_assertions)]
#[test]
fn an_attached_client_may_stay_silent_past_the_attach_deadline() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::listen_mode_with("run", &path, &[ATTACH_DEADLINE]);
    client.initialize();
    client.attach(false);
    std::thread::sleep(Duration::from_millis(600));
    client.configuration_done();
    assert_eq!(exit_code_reported(&mut client), 0);
    let finished = client.finish();
    assert_eq!(finished.status, Some(0), "{}", finished.stderr);
    assert_eq!(finished.stdout, "hi\n");
}

/// A peer that never attaches -- here one that trickles `initialize`
/// requests, each well inside the deadline -- is closed at the deadline,
/// and the next client is served.
#[cfg(debug_assertions)]
#[test]
fn a_peer_that_never_attaches_is_closed_at_the_deadline() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::listen_mode_with("run", &path, &[ATTACH_DEADLINE]);
    let started = std::time::Instant::now();
    // Requests every 50 ms, which a read timeout would never end.
    while started.elapsed() < Duration::from_secs(5) {
        let Ok(seq) = client.connection.try_send("initialize", json!({})) else {
            break;
        };
        if !client.connection.answers_or_closes(seq) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the connection outlived the deadline"
    );
    client.reconnect();
    client.start_session(false);
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().status, Some(0));
}

/// A root whose `main:args:` holds an Integer and an Array in its locals
/// before line 6 prints the Integer.
const STOPS: &str = indoc! {"
    class Stops = Object new () (
      public main: platform args: arguments = (
        | count elements |
        count:: 3.
        elements:: {10. 20. 30}.
        platform transcript show: count printString; newLine.
      )
    ) : ()
"};

/// The DAP source of the program file at `path`.
fn file_source(path: &Path) -> Value {
    json!({ "path": absolute(path) })
}

fn absolute(path: &Path) -> String {
    std::path::absolute(path)
        .expect("an absolute path")
        .display()
        .to_string()
}

/// Starts a session that stops at the entry, sets breakpoints at the start
/// of `lines` of the program file `path` there and goes on; answers the
/// breakpoints. Set at the entry, since the VM serves a configuration
/// command only once a run starts, and the first starts after
/// `configurationDone`.
fn breakpoints_from_the_entry(client: &mut DapClient, path: &Path, lines: &[u32]) -> Vec<Value> {
    client.start_session(true);
    client.stopped("entry");
    let breakpoints = client.set_breakpoints(file_source(path), lines);
    client.succeed("continue", json!({ "threadId": 1 }));
    breakpoints
}

/// The variable named `name` among `variables`.
fn variable<'a>(variables: &'a [Value], name: &str) -> &'a Value {
    variables
        .iter()
        .find(|variable| variable["name"] == name)
        .unwrap_or_else(|| panic!("no variable `{name}`: {variables:?}"))
}

/// The `Locals` scope's reference of the frame `frame`.
fn locals_of(client: &mut DapClient, frame: &Value) -> Value {
    let scopes = client.succeed("scopes", json!({ "frameId": frame["id"] }));
    let scopes = dap::array(&scopes["scopes"]);
    assert_eq!(scopes[0]["name"], "Locals", "{scopes:?}");
    scopes[0]["variablesReference"].clone()
}

#[test]
fn a_breakpoint_stop_shows_its_frame_and_variables() {
    let (_directory, path) = program("Stops", STOPS);
    let mut client = DapClient::connect_mode(&path);
    let breakpoints = breakpoints_from_the_entry(&mut client, &path, &[6]);
    assert_eq!(breakpoints[0]["verified"], true, "{breakpoints:?}");
    assert_eq!(breakpoints[0]["line"], 6, "{breakpoints:?}");
    let stopped = client.stopped("breakpoint");
    assert_eq!(
        stopped["body"]["hitBreakpointIds"],
        json!([breakpoints[0]["id"]]),
        "{stopped}"
    );
    assert_eq!(stopped["body"]["allThreadsStopped"], true, "{stopped}");
    let threads = client.succeed("threads", json!({}));
    assert_eq!(threads["threads"], json!([{ "id": 1, "name": "main" }]));

    let frames = client.stack_frames(0, 0);
    let top = &frames[0];
    assert_eq!(top["id"], 1, "{top}");
    assert_eq!(top["name"], "Stops>>main:args:", "{top}");
    assert_eq!(top["line"], 6, "{top}");
    assert_eq!(top["column"], 5, "{top}");
    assert_eq!(top["source"]["name"], "Stops.ns", "{top}");
    assert_eq!(top["source"]["path"], absolute(&path), "{top}");
    assert!(top["source"].get("sourceReference").is_none(), "{top}");

    let locals = locals_of(&mut client, top);
    let variables = client.variables(&locals, None, None);
    let count = variable(&variables, "count");
    assert_eq!(count["value"], "3", "{count}");
    assert_eq!(count["variablesReference"], 0, "{count}");
    let elements = variable(&variables, "elements");
    assert_ne!(elements["variablesReference"], 0, "{elements}");
    assert_eq!(elements["indexedVariables"], 3, "{elements}");
    let page = client.variables(&elements["variablesReference"], Some(1), Some(2));
    let page: Vec<(Value, Value)> = page
        .iter()
        .map(|element| (element["name"].clone(), element["value"].clone()))
        .collect();
    assert_eq!(
        page,
        [(json!("[2]"), json!("20")), (json!("[3]"), json!("30"))]
    );

    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    let finished = client.finish();
    assert_eq!(finished.status, Some(0), "{}", finished.stderr);
    assert_eq!(finished.stdout, "3\n");
}

/// A root whose `main:args:` sends `report:` on a line that holds
/// characters outside the Basic Multilingual Plane before the send.
const NON_ASCII: &str = indoc! {"
    class Wide = Object new () (
      public main: platform args: arguments = (
        | text |
        text:: 'ü😀'. self report: text.
      )
      report: text = (
        text size printString.
      )
    ) : ()
"};

#[test]
fn a_frame_after_non_ascii_text_reports_its_line_and_column() {
    let (_directory, path) = program("Wide", NON_ASCII);
    let mut client = DapClient::connect_mode(&path);
    let breakpoints = breakpoints_from_the_entry(&mut client, &path, &[7]);
    assert_eq!(breakpoints[0]["verified"], true, "{breakpoints:?}");
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 0);
    assert_eq!(frames[0]["name"], "Wide>>report:", "{frames:?}");
    assert_eq!(frames[0]["line"], 7, "{frames:?}");
    let caller = &frames[1];
    let line = NON_ASCII.lines().nth(3).expect("the fourth line");
    let before = &line[..line.find("report:").expect("the send")];
    let column = before.encode_utf16().count() + 1;
    assert_eq!(caller["line"], 4, "{caller}");
    assert_eq!(caller["column"], column, "{caller}");
    assert_eq!(caller["endLine"], 4, "{caller}");
    assert_eq!(caller["endColumn"], column + "report:".len(), "{caller}");
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
}

/// A root whose first statement sends `do:` to an Array, a Kernel method.
const WALKER: &str = indoc! {"
    class Walker = Object new () (
      public main: platform args: arguments = (
        {1. 2} do: [:each | platform transcript show: each printString].
        platform transcript newLine.
      )
    ) : ()
"};

#[test]
fn a_shipped_frame_is_shown_through_a_source_reference() {
    let (_directory, path) = program("Walker", WALKER);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(true);
    client.stopped("entry");
    client.succeed(
        "nsNext/setStepFilters",
        json!({ "shipped": false, "classes": [] }),
    );
    client.succeed("stepIn", json!({ "threadId": 1 }));
    client.stopped("step");
    let frames = client.stack_frames(0, 1);
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0]["name"], "Array>>do:", "{frames:?}");
    let source = frames[0]["source"].clone();
    assert_eq!(source["name"], "Kernel.ns", "{source}");
    assert!(source.get("path").is_none(), "{source}");
    let reference = source["sourceReference"].clone();
    assert!(reference.as_i64().is_some_and(|id| id > 0), "{source}");
    let text = client.succeed("source", json!({ "sourceReference": reference }));
    let kernel = std::fs::read_to_string(
        common::repository_root().join("crates/ns-runtime/core-library/Kernel.ns"),
    )
    .expect("read Kernel.ns");
    assert!(text["content"] == kernel.as_str(), "not Kernel.ns's text");

    client.succeed("stepIn", json!({ "threadId": 1 }));
    client.stopped("step");
    let frames = client.stack_frames(0, 1);
    assert_eq!(frames[0]["name"], "Array>>do:", "{frames:?}");
    assert_eq!(frames[0]["source"]["sourceReference"], reference);
    let statistics = client.succeed("nsNext/statistics", json!({}));
    assert_eq!(statistics, json!({ "moduleTexts": 1 }));

    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "12\n");
}

/// A root whose loop passes line 6 five times.
const LOOP: &str = indoc! {"
    class Loop = Object new () (
      public main: platform args: arguments = (
        | total |
        total:: 0.
        1 to: 5 do: [:index |
          total:: total + index.
        ].
        platform transcript show: total printString; newLine.
      )
    ) : ()
"};

/// A breakpoint replaced while the program goes on to it: the stop that
/// reaches it names, at most, the breakpoint that replaced it, whichever of
/// the stop and the replacement the VM served first; and a variables
/// reference of the earlier stop is refused.
#[test]
fn a_stop_naming_a_replaced_breakpoint_is_reported_without_it() {
    let (_directory, path) = program("Loop", LOOP);
    let mut client = DapClient::connect_mode(&path);
    let first = breakpoints_from_the_entry(&mut client, &path, &[6]);
    let stopped = client.stopped("breakpoint");
    assert_eq!(stopped["body"]["hitBreakpointIds"], json!([first[0]["id"]]));
    let frames = client.stack_frames(0, 0);
    let stale = locals_of(&mut client, &frames[0]);

    let continue_seq = client.connection.send("continue", json!({ "threadId": 1 }));
    let replace_seq = client.connection.send(
        "setBreakpoints",
        json!({ "source": file_source(&path), "breakpoints": [{ "line": 6 }] }),
    );
    let continued = client.connection.response_to(continue_seq, "continue");
    assert_eq!(continued["success"], true, "{continued}");
    let replaced = client.connection.response_to(replace_seq, "setBreakpoints");
    let second = &replaced["body"]["breakpoints"][0];
    assert_ne!(second["id"], first[0]["id"], "{replaced}");
    let stopped = client.stopped("breakpoint");
    if let Some(ids) = stopped["body"].get("hitBreakpointIds") {
        assert_eq!(ids, &json!([second["id"]]), "{stopped}");
    }
    let refused = client.request("variables", json!({ "variablesReference": stale }));
    assert_eq!(refused["success"], false, "{refused}");

    client.set_breakpoints(file_source(&path), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "15\n");
}

/// `STOPS` built, with the build's `options`, into an image in a scratch
/// directory; answers the directory and the image's path.
fn built_image(options: &[&str]) -> (ScratchDirectory, PathBuf) {
    let (directory, path) = program("Stops", STOPS);
    let image = directory.join("stops.nsi");
    let mut arguments: Vec<&OsStr> = vec!["build".as_ref(), path.as_os_str(), image.as_os_str()];
    arguments.extend(options.iter().map(OsStr::new));
    let (built, _, error) = common::ns_next(&arguments);
    assert!(built, "{error}");
    (directory, image)
}

#[test]
fn a_loaded_image_shows_its_modules_through_source_references() {
    let (_directory, image) = built_image(&[]);
    let mut client = DapClient::connect_mode_of("load", &image);
    client.start_session(true);
    client.stopped("entry");
    let frames = client.stack_frames(0, 0);
    let source = frames[0]["source"].clone();
    assert_eq!(source["name"], "Stops.ns", "{source}");
    assert!(source.get("path").is_none(), "{source}");
    let breakpoints = client.set_breakpoints(json!({ "name": source["name"] }), &[6]);
    assert_eq!(breakpoints[0]["verified"], true, "{breakpoints:?}");
    client.succeed("continue", json!({ "threadId": 1 }));
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 0);
    assert_eq!(frames[0]["line"], 6, "{frames:?}");
    let text = client.succeed(
        "source",
        json!({ "sourceReference": frames[0]["source"]["sourceReference"] }),
    );
    assert_eq!(text["content"], STOPS);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "3\n");
}

#[test]
fn a_stripped_image_shows_frames_without_lines() {
    let (_directory, image) = built_image(&["--strip-source"]);
    let mut client = DapClient::connect_mode_of("load", &image);
    client.start_session(true);
    client.stopped("entry");
    // A body without metadata stops at its first safepoint rather than its
    // first statement: here the push of `transcript`, shipped code.
    let frames = client.stack_frames(0, 0);
    let main = frames
        .iter()
        .find(|frame| frame["name"] == "Stops>>main:args:")
        .unwrap_or_else(|| panic!("no frame of `main:args:`: {frames:?}"));
    assert_eq!(main["line"], 0, "{main}");
    assert_eq!(main["column"], 0, "{main}");
    assert!(main.get("source").is_none(), "{main}");
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
}
