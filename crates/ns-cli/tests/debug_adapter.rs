//! `ns-next run --debug` and `--debug-connect`: the agent's connection,
//! its session and its link to the host's runs, through a DAP client
//! (`common/dap.rs`) against a child `ns-next`.

mod common;
#[path = "common/dap.rs"]
mod dap;

use std::path::PathBuf;
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
    // The pause stops the send's first safepoint, which is the first push
    // `main:args:` makes, not its own entry: the stop stands in
    // `main:args:`'s run, with its frame at the bottom of the stack.
    let frames = trace["body"]["stackFrames"]
        .as_array()
        .expect("the stack's frames");
    assert_eq!(
        frames.last().map(|frame| &frame["name"]),
        Some(&json!("Hello>>main:args:")),
        "{trace}"
    );
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
    // A port nothing listens on: bound, then given back.
    let address = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("a free port");
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
