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
use indoc::{formatdoc, indoc};
use serde_json::{Value, json};

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

/// A project's root whose initializer runs a while before `main:args:`:
/// longer than any machine takes to deliver a `terminate` sent right after
/// `configurationDone`, which machine code would outrun with a short loop,
/// and no longer than that, since the terminate ends the run at its next
/// safepoint, or the host's check ends the program before the next run.
const SLOW_START: &str = indoc! {"
    class SlowStart packageUsing: manifest = Object new (
      |
        count ::= 0.
      |
      1 to: 1000000000 do: [:index | count:: count + 1].
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

/// A scratch directory holding the binary's shipped sources, as `ns-next
/// shipped-sources` writes them for a client's core library, and the
/// directory's absolute path.
fn core_library() -> (ScratchDirectory, PathBuf) {
    let directory = ScratchDirectory::new("core-library");
    let library = directory.join("library");
    let (written, _, error) =
        common::ns_next(&[OsStr::new("shipped-sources"), library.as_os_str()]);
    assert!(written, "{error}");
    let library = PathBuf::from(absolute(&library));
    (directory, library)
}

/// The 1-based line of `Array>>do:`'s loop body in the shipped
/// `Kernel.ns`, which a program's `do:` over an array reaches.
fn array_do_body_line() -> u32 {
    let kernel = ns_runtime::shipped_module_source("Kernel").expect("the shipped kernel");
    let lines: Vec<&str> = kernel.lines().collect();
    let size = lines
        .iter()
        .position(|line| line.contains("count = vmMirror collectionSize: self ifFail:"))
        .expect("Array>>do:");
    let body = size
        + lines[size..]
            .iter()
            .position(|line| line.trim() == "action value: (self at: index).")
            .expect("its loop body");
    u32::try_from(body + 1).expect("a line number")
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
    assert!(
        process["body"].get("nsNextSecret").is_none(),
        "a process given no secret returns none: {process}"
    );
    client.configuration_done();
    assert_eq!(exit_code_reported(&mut client), 0);
    let finished = client.finish();
    assert_eq!(finished.status, Some(0), "{}", finished.stderr);
    assert_eq!(finished.stdout, "hi\n");
    assert_eq!(finished.stderr, "");
}

/// The connecting mode returns the secret its launcher handed it in
/// `NS_NEXT_DEBUG_SECRET` in the `process` event, so that the launcher can
/// tell its own process from another that connected first, whose process
/// id it could name; the listening mode, whose client presents a token,
/// returns none.
#[test]
fn the_process_event_returns_the_launcher_s_secret() {
    let (_directory, path) = program("Hello", HELLO);
    let secret = "0123456789abcdef0123456789abcdef";
    let mut client =
        DapClient::connect_mode_with("run", &path, &[("NS_NEXT_DEBUG_SECRET", secret)]);
    client.initialize();
    let process = client.attach(false);
    assert_eq!(process["body"]["nsNextSecret"], secret, "{process}");
    client.configuration_done();
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().status, Some(0));

    let mut client = DapClient::listen_mode_with("run", &path, &[("NS_NEXT_DEBUG_SECRET", secret)]);
    client.initialize();
    let process = client.attach(false);
    assert!(process["body"].get("nsNextSecret").is_none(), "{process}");
    client.configuration_done();
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().status, Some(0));
}

/// Every request the agent receives is answered, the ones that arrive
/// after the program ended included -- as a `terminate` sent just before
/// the end may: the agent stays to answer until the client disconnects.
/// A `terminate` then succeeds, the program having ended, and a request
/// that needs the program is refused.
#[test]
fn a_request_after_the_program_ended_is_answered() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(false);
    let exited = client.event("exited");
    assert_eq!(exited["body"]["exitCode"], 0, "{exited}");
    client.event("terminated");
    let terminated = client.request("terminate", json!({}));
    assert_eq!(terminated["success"], true, "{terminated}");
    let threads = client.request("stackTrace", json!({ "threadId": 1 }));
    assert_eq!(threads["success"], false, "{threads}");
    let disconnected = client.request("disconnect", json!({}));
    assert_eq!(disconnected["success"], true, "{disconnected}");
    client.connection.expect_closed();
    let finished = client.finish();
    assert_eq!(finished.status, Some(0), "{}", finished.stderr);
    assert_eq!(finished.stdout, "hi\n");
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

/// A connecting agent's launcher owns the process: a connection lost
/// before a `disconnect` ends the program as `terminate` does, its
/// `ensure:` blocks run, with a terminated program's status -- the
/// program loops forever otherwise. A stopped program goes on to its end
/// the same way.
#[test]
fn a_lost_connection_ends_the_program_in_the_connecting_mode() {
    let (_directory, path) = program("Looper", LOOPER);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(false);
    client.wait_for_stdout("looping\n");
    client.connection.close();
    let finished = client.finish();
    assert_eq!(finished.status, Some(1), "{}", finished.stderr);
    assert!(
        finished.stdout.ends_with("ensured\n"),
        "{}",
        finished.stdout
    );
    assert!(finished.stderr.contains(TERMINATED), "{}", finished.stderr);

    let (_directory, path) = program("Looper", LOOPER);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(true);
    client.stopped("entry");
    client.connection.close();
    let finished = client.finish();
    assert_eq!(finished.status, Some(1), "{}", finished.stderr);
    assert!(!finished.stdout.contains("looping"), "{}", finished.stdout);
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

/// A listening agent clears what a client that left set: the next client
/// meets none of its line or function breakpoints.
#[test]
fn a_second_client_meets_none_of_the_first_s_breakpoints() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::listen_mode(&path);
    client.initialize();
    client.attach(false);
    let breakpoints = client.set_breakpoints(file_source(&path), &[3]);
    assert_eq!(breakpoints[0]["verified"], true, "{breakpoints:?}");
    let functions = client.succeed(
        "setFunctionBreakpoints",
        json!({ "breakpoints": [{ "name": "Hello>>main:args:" }] }),
    );
    assert_eq!(functions["breakpoints"][0]["verified"], true, "{functions}");
    client.succeed("disconnect", json!({}));
    client.connection.expect_closed();

    client.reconnect();
    client.start_session(false);
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "hi\n");
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
const ATTACH_DEADLINE: (&str, &str) = ("NS_NEXT_DEBUG_ATTACH_DEADLINE_MILLISECONDS", "500");

/// `ATTACH_DEADLINE`'s value.
#[cfg(debug_assertions)]
const ATTACH_DEADLINE_DURATION: Duration = Duration::from_millis(500);

/// How many connections a client tries before it attaches in time. The
/// deadline is short so that the tests are quick, and a machine running
/// many suites at once may let a connection's deadline pass before its
/// first request is read; a connection the deadline closed is the
/// behaviour under test, and a fresh one is tried.
#[cfg(debug_assertions)]
const ATTACH_ATTEMPTS: u32 = 10;

/// The deadline to attach ends nothing once the client attached: a client
/// silent for three times the deadline after its `attach` keeps its
/// session.
#[cfg(debug_assertions)]
#[test]
fn an_attached_client_may_stay_silent_past_the_attach_deadline() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::listen_mode_with("run", &path, &[ATTACH_DEADLINE]);
    client.attach_racing_the_deadline(ATTACH_ATTEMPTS);
    std::thread::sleep(ATTACH_DEADLINE_DURATION * 3);
    client.configuration_done();
    assert_eq!(exit_code_reported(&mut client), 0);
    let finished = client.finish();
    assert_eq!(finished.status, Some(0), "{}", finished.stderr);
    assert_eq!(finished.stdout, "hi\n");
}

/// A peer that never attaches -- here one that trickles `initialize`
/// requests every 50 ms, which no read timeout would ever end -- is closed
/// once the deadline passed, whatever it was answered meanwhile, and the
/// next client is served.
#[cfg(debug_assertions)]
#[test]
fn a_peer_that_never_attaches_is_closed_at_the_deadline() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::listen_mode_with("run", &path, &[ATTACH_DEADLINE]);
    let started = std::time::Instant::now();
    // Generous on a loaded machine; a read timeout would hold it forever.
    let bound = ATTACH_DEADLINE_DURATION + Duration::from_secs(5);
    while started.elapsed() < bound {
        if client.connection.try_send("initialize", json!({})).is_err()
            || client.connection.closed_now()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // A write may fail at the close before the reader has seen it: the
    // closed state is waited for, as long again.
    while !client.connection.closed_now() && started.elapsed() < bound * 2 {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        client.connection.closed_now(),
        "the connection outlived the deadline by more than 5 s"
    );
    client.reconnect();
    client.attach_racing_the_deadline(ATTACH_ATTEMPTS);
    client.configuration_done();
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().status, Some(0));
}

/// A peer that floods the agent with requests and never reads the answers
/// cannot keep the agent from its deadline: the agent's writes to it would
/// otherwise fill the socket's buffers and block the agent, which then
/// never served the deadline, nor the next client waiting behind it.
#[cfg(debug_assertions)]
#[test]
fn a_peer_that_never_reads_is_closed_at_the_deadline() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::listen_mode_with("run", &path, &[ATTACH_DEADLINE]);
    // The first connection leaves at once, so that the agent takes the
    // unread one next.
    client.connection.close();
    let mut unread = client.connect_unread();
    let frame = |seq: usize| {
        let body = json!({ "seq": seq, "type": "request", "command": "initialize" }).to_string();
        format!("Content-Length: {}\r\n\r\n{body}", body.len())
    };
    // Far more answers than any socket buffers: the agent's writes block
    // unless it bounds them.
    let flood: String = (1..=20_000).map(frame).collect();
    let _ = std::io::Write::write_all(&mut unread, flood.as_bytes());
    client.reconnect();
    client.attach_racing_the_deadline(ATTACH_ATTEMPTS);
    client.configuration_done();
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().status, Some(0));
    drop(unread);
}

/// An attached client that stops reading cannot wedge the agent: its
/// writes are bounded and its queue is, so the agent ends its session as a
/// disconnect does -- in the listening mode, the VM reset and the next
/// client accepted -- and the next client runs the program to its end.
/// The client here attaches, presenting the token, then floods the agent
/// with requests and reads nothing at all.
#[cfg(debug_assertions)]
#[test]
fn an_attached_client_that_stops_reading_is_dropped() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::listen_mode_with(
        "run",
        &path,
        &[
            ("NS_NEXT_DEBUG_ATTACH_DEADLINE_MILLISECONDS", "5000"),
            ("NS_NEXT_DEBUG_WRITE_BOUND_MILLISECONDS", "500"),
        ],
    );
    client.connection.close();
    let mut unread = client.connect_unread();
    let frame = |seq: usize, command: &str, arguments: Value| {
        let body = json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        })
        .to_string();
        format!("Content-Length: {}\r\n\r\n{body}", body.len())
    };
    let mut requests = frame(1, "attach", json!({ "token": client.token() }));
    // Far more answers than the socket's buffers and the agent's queue
    // hold on any platform: an `initialize`'s answer, its capabilities,
    // is some 700 bytes, so these are some 14 MB.
    for seq in 2..=20_000 {
        requests.push_str(&frame(seq, "initialize", json!({})));
    }
    let started = std::time::Instant::now();
    let _ = std::io::Write::write_all(&mut unread, requests.as_bytes());
    client.reconnect();
    client.attach_racing_the_deadline(ATTACH_ATTEMPTS);
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "the next client waited {:?}",
        started.elapsed()
    );
    client.configuration_done();
    assert_eq!(exit_code_reported(&mut client), 0);
    let finished = client.finish();
    assert_eq!(finished.status, Some(0), "{}", finished.stderr);
    assert_eq!(finished.stdout, "hi\n");
    drop(unread);
}

/// A peer that has not attached is read no frame longer than 64 KiB, so
/// that whoever reaches the listening socket cannot make the program
/// parse a message of the length an attached client may send: its
/// connection is closed unanswered, and the next client is served, which
/// once attached may send a frame far longer.
#[test]
fn an_unattached_peer_s_long_frame_closes_its_connection() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::listen_mode(&path);
    let padding = "x".repeat(1024 * 1024);
    // The agent may close the connection before the whole frame is
    // written, which fails the write.
    let answered = client
        .connection
        .try_send("initialize", json!({ "clientID": padding }))
        .ok()
        .and_then(|seq| client.connection.response_or_close(seq));
    assert!(
        answered.is_none(),
        "a long frame was answered: {answered:?}"
    );

    client.reconnect();
    client.initialize();
    client.attach(false);
    let long = "x".repeat(256 * 1024);
    let threads = client.request("threads", json!({ "padding": long }));
    assert_eq!(threads["success"], true, "{threads}");
    client.configuration_done();
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().status, Some(0));
}

/// The linger after `terminated` is a deadline from that event, not a
/// bound on the client's silence: a client that keeps sending requests
/// and never disconnects is answered until it passes, then the process
/// ends with the program's status. A debug build reads the linger from a
/// variable, so that the test takes a fraction of a second.
#[cfg(debug_assertions)]
#[test]
fn a_client_that_never_disconnects_cannot_keep_the_ended_program_s_process() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::connect_mode_with(
        "run",
        &path,
        &[("NS_NEXT_DEBUG_ENDED_LINGER_MILLISECONDS", "500")],
    );
    client.start_session(false);
    client.event("terminated");
    let started = std::time::Instant::now();
    // Every 100 ms, well inside the linger, for far longer than it.
    while started.elapsed() < Duration::from_secs(10) {
        if client.connection.try_send("threads", json!({})).is_err()
            || client.connection.closed_now()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the process outlived its program by {:?}",
        started.elapsed()
    );
    let finished = client.finish();
    assert_eq!(finished.status, Some(0), "{}", finished.stderr);
    assert_eq!(finished.stdout, "hi\n");
}

/// A project's root that reaches `Shared` through its manifest.
const SHARING: &str = indoc! {"
    class App packageUsing: manifest = Object new (
      | Shared = manifest at: 'Shared'. |
    ) (
      public main: platform args: arguments = (
        platform transcript show: Shared new greeting; newLine.
      )
    ) : ()
"};

/// A module whose `greeting` answers on line 3.
const SHARED: &str = indoc! {"
    class Shared = Object new () (
      public greeting = (
        ^'shared'
      )
    ) : ()
"};

/// A project whose module lies beside the project's directory, named
/// `../shared/Shared.ns` in its `newspeak.json`, as written: a breakpoint
/// the client sets by the module's plain path, as an IDE names the file,
/// stops there.
#[test]
fn a_module_named_through_dot_dot_takes_a_breakpoint_by_its_plain_path() {
    let directory = ScratchDirectory::new("SharedModule");
    let project = directory.join("app");
    let shared = directory.join("shared");
    std::fs::create_dir_all(&project).expect("make the project's directory");
    std::fs::create_dir_all(&shared).expect("make the module's directory");
    std::fs::write(project.join("App.ns"), SHARING).expect("write the root");
    std::fs::write(shared.join("Shared.ns"), SHARED).expect("write the module");
    std::fs::write(
        project.join("newspeak.json"),
        r#"{ "main": "App.ns", "modules": ["../shared/Shared.ns"] }"#,
    )
    .expect("write the project file");
    let mut client = DapClient::connect_mode(&project);
    let breakpoints = breakpoints_before_the_start(&mut client, &shared.join("Shared.ns"), &[3]);
    assert_eq!(breakpoints[0]["verified"], true, "{breakpoints:?}");
    client.stopped("breakpoint");
    assert_eq!(top_frame(&mut client).1, 3);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "shared\n");
}

/// Makes `link` a link to the directory `target`: a symbolic link, or on
/// Windows a junction, which needs no privilege.
fn link_directory(target: &Path, link: &Path) {
    #[cfg(windows)]
    {
        let made = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .output()
            .expect("run mklink");
        assert!(
            made.status.success(),
            "mklink /J: {}",
            String::from_utf8_lossy(&made.stderr)
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).expect("make a symbolic link");
}

/// A program started through a link to its directory takes a breakpoint
/// the client sets by the file's real path, which is how an IDE names a
/// file it resolved.
#[test]
fn a_program_started_through_a_link_takes_a_breakpoint_by_its_real_path() {
    let directory = ScratchDirectory::new("LinkedProgram");
    let real = directory.join("real");
    std::fs::create_dir_all(&real).expect("make the program's directory");
    std::fs::write(real.join("Stops.ns"), STOPS).expect("write the program");
    let link = directory.join("link");
    link_directory(&real, &link);
    let mut client = DapClient::connect_mode(&link.join("Stops.ns"));
    let breakpoints = breakpoints_before_the_start(&mut client, &real.join("Stops.ns"), &[6]);
    assert_eq!(breakpoints[0]["verified"], true, "{breakpoints:?}");
    client.stopped("breakpoint");
    assert_eq!(top_frame(&mut client).1, 6);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "3\n");
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

/// Starts a session in DAP's order: breakpoints at the start of `lines` of
/// the program file `path`, their response awaited, then
/// `configurationDone`; answers the breakpoints.
fn breakpoints_before_the_start(client: &mut DapClient, path: &Path, lines: &[u32]) -> Vec<Value> {
    client.initialize();
    client.attach(false);
    let breakpoints = client.set_breakpoints(file_source(path), lines);
    client.configuration_done();
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
    let breakpoints = breakpoints_before_the_start(&mut client, &path, &[6]);
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
    let breakpoints = breakpoints_before_the_start(&mut client, &path, &[7]);
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

/// Whether `bytecode` holds the line of the instruction a frame stands
/// at: its index followed at once by `->`.
fn marks_an_instruction(bytecode: &str) -> bool {
    bytecode.lines().any(|line| {
        line.split_whitespace()
            .next()
            .is_some_and(|word| word.ends_with("->") && word.len() > 2)
    })
}

#[test]
fn frame_code_marks_the_stopped_instruction() {
    let (_directory, path) = program("Wide", NON_ASCII);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[7]);
    client.stopped("breakpoint");
    let code = client.succeed("nsNext/frameCode", json!({ "frameId": 1 }));
    assert_eq!(code["body"], "Wide>>report:", "{code}");
    let bytecode = code["bytecode"].as_str().unwrap_or_default();
    assert!(marks_an_instruction(bytecode), "{code}");
    // The top frame stands at the stop's safepoint, never at a call its
    // machine code returns from.
    assert_eq!(code["machineCode"], Value::Null, "{code}");
    // The caller is marked at the send it waits on.
    let caller = client.succeed("nsNext/frameCode", json!({ "frameId": 2 }));
    assert_eq!(caller["body"], "Wide>>main:args:", "{caller}");
    let bytecode = caller["bytecode"].as_str().unwrap_or_default();
    assert!(marks_an_instruction(bytecode), "{caller}");

    let refused = client.request("nsNext/frameCode", json!({ "frameId": 9 }));
    assert_eq!(refused["success"], false, "{refused}");
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
}

/// A root whose `outer:`, sent 50 times, sends `report:`, whose second
/// statement, line 11, a breakpoint stands on: past the first, so that
/// `report:`'s body, compiled again after the patch freed its code, is
/// called from `outer:`'s machine code rather than sent by its exit.
const HOT_CALLER: &str = indoc! {"
    class Hot = Object new () (
      public main: platform args: arguments = (
        1 to: 50 do: [:index | self outer: index].
      )
      outer: index = (
        ^self report: index
      )
      report: index = (
        | total |
        total:: self double: index.
        ^total + 1
      )
      double: index = (
        ^index + index
      )
    ) : ()
"};

#[test]
fn frame_code_marks_where_a_compiled_caller_returns() {
    let (_directory, path) = program("Hot", HOT_CALLER);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    client.succeed(
        "setBreakpoints",
        json!({
            "source": file_source(&path),
            "breakpoints": [{ "line": 11, "hitCondition": "40" }],
        }),
    );
    client.configuration_done();
    client.stopped("breakpoint");
    let caller = client.succeed("nsNext/frameCode", json!({ "frameId": 2 }));
    assert_eq!(caller["body"], "Hot>>outer:", "{caller}");
    // `outer:` runs its machine code from its second entry on, where a
    // JIT compiles it; without one it goes on interpreted.
    if let Some(machine_code) = caller["machineCode"].as_str() {
        assert!(
            machine_code.starts_with("--- code {Hot>>outer:} baseline "),
            "{machine_code}"
        );
        assert!(machine_code.ends_with("--- end\n"), "{machine_code}");
        assert_eq!(
            machine_code
                .lines()
                .filter(|line| line.starts_with("  ->+0x"))
                .count(),
            1,
            "{machine_code}"
        );
    }
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
    client.initialize();
    client.attach(true);
    client.succeed(
        "nsNext/setStepFilters",
        json!({ "shipped": false, "entries": [] }),
    );
    client.configuration_done();
    client.stopped("entry");
    client.succeed("stepIn", json!({ "threadId": 1 }));
    client.stopped("step");
    let frames = client.stack_frames(0, 1);
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(
        frames[0]["name"], "newspeak.core.Kernel/Array>>do:",
        "{frames:?}"
    );
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
    // Every frame: the program's module's text is asked of the VM, a
    // shipped module's never is.
    client.stack_frames(0, 0);

    client.succeed("stepIn", json!({ "threadId": 1 }));
    client.stopped("step");
    let frames = client.stack_frames(0, 0);
    assert_eq!(
        frames[0]["name"], "newspeak.core.Kernel/Array>>do:",
        "{frames:?}"
    );
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
/// reference of the earlier stop is refused. A breakpoint the client sends
/// again keeps its id.
#[test]
fn a_stop_naming_a_replaced_breakpoint_is_reported_without_it() {
    let (_directory, path) = program("Loop", LOOP);
    let mut client = DapClient::connect_mode(&path);
    let first = breakpoints_before_the_start(&mut client, &path, &[6]);
    let stopped = client.stopped("breakpoint");
    assert_eq!(stopped["body"]["hitBreakpointIds"], json!([first[0]["id"]]));
    let frames = client.stack_frames(0, 0);
    let stale = locals_of(&mut client, &frames[0]);
    let again = client.set_breakpoints(file_source(&path), &[6]);
    assert_eq!(again[0]["id"], first[0]["id"], "{again:?}");

    // Sent in one write, so that the replacement is on its way before any
    // stop the `continue` makes. That does not make the agent's reader
    // hand it over before the stop is reported: a request still on its
    // way then is one DAP lets cross the stop.
    let seqs = client.connection.send_together(&[
        ("continue", json!({ "threadId": 1 })),
        (
            "setBreakpoints",
            json!({ "source": file_source(&path), "breakpoints": [{ "line": 8 }] }),
        ),
    ]);
    let continued = client.connection.response_to(seqs[0], "continue");
    assert_eq!(continued["success"], true, "{continued}");
    let replaced = client.connection.response_to(seqs[1], "setBreakpoints");
    let second = &replaced["body"]["breakpoints"][0];
    assert_ne!(second["id"], first[0]["id"], "{replaced}");
    let stopped = client.stopped("breakpoint");
    let refused = client.request("variables", json!({ "variablesReference": stale }));
    assert_eq!(refused["success"], false, "{refused}");
    // A stop the VM decided before it served the replacement stands on
    // line 6. It names no breakpoint when the agent had the replacement
    // as it reported the stop, the client having replaced line 6's, and
    // line 6's when the replacement crossed the stop; never any other.
    // Which of the two is a race, the rule itself pinned by
    // `a_pending_replacement_hides_the_ids_it_replaces`. The
    // replacement's on line 8 follows it.
    if top_frame(&mut client).1 == 6 {
        let named = stopped["body"].get("hitBreakpointIds");
        assert!(
            named.is_none() || named == Some(&json!([first[0]["id"]])),
            "{stopped}"
        );
        client.succeed("continue", json!({ "threadId": 1 }));
        let stopped = client.stopped("breakpoint");
        assert_eq!(
            stopped["body"]["hitBreakpointIds"],
            json!([second["id"]]),
            "{stopped}"
        );
    } else {
        assert_eq!(
            stopped["body"]["hitBreakpointIds"],
            json!([second["id"]]),
            "{stopped}"
        );
    }
    assert_eq!(top_frame(&mut client).1, 8);

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

/// DAP's order -- a `setBreakpoints` answered before `configurationDone`
/// is sent -- holds: the host serves the configuration while it waits.
#[test]
fn a_breakpoint_set_before_configuration_done_is_answered_and_stops() {
    let (_directory, path) = program("Hello", HELLO);
    let mut client = DapClient::connect_mode(&path);
    let breakpoints = breakpoints_before_the_start(&mut client, &path, &[3]);
    assert_eq!(breakpoints[0]["verified"], true, "{breakpoints:?}");
    assert_eq!(breakpoints[0]["line"], 3, "{breakpoints:?}");
    client.stopped("breakpoint");
    assert_eq!(client.stdout(), "");
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "hi\n");
}

/// A breakpoint in the core library's `Kernel.ns`, named by that file's
/// path, stops in the shipped `Array>>do:`, whose frame is shown by the
/// same path.
#[test]
fn a_breakpoint_in_a_shipped_module_set_before_the_start_stops() {
    let (_directory, path) = program("Walker", WALKER);
    let (_library_directory, library) = core_library();
    let kernel = file_source(&library.join("Kernel.ns"));
    let line = array_do_body_line();
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach_with(json!({ "stopOnEntry": false, "coreLibrary": absolute(&library) }));
    let breakpoints = client.set_breakpoints(kernel.clone(), &[line]);
    assert_eq!(breakpoints[0]["verified"], true, "{breakpoints:?}");
    assert_eq!(breakpoints[0]["line"], line, "{breakpoints:?}");
    client.configuration_done();
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 1);
    assert_eq!(
        frames[0]["name"], "newspeak.core.Kernel/Array>>do:",
        "{frames:?}"
    );
    assert_eq!(frames[0]["line"], line, "{frames:?}");
    assert_eq!(frames[0]["source"]["name"], "Kernel.ns", "{frames:?}");
    assert_eq!(frames[0]["source"]["path"], kernel["path"], "{frames:?}");
    assert!(
        frames[0]["source"].get("sourceReference").is_none(),
        "{frames:?}"
    );
    client.set_breakpoints(kernel, &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "12\n");
}

/// A program whose own root is named `Kernel`, like the shipped module:
/// its line 4 runs a `do:` over an array, which the shipped `Array>>do:`
/// runs, and its line 5 prints the sum.
const OWN_KERNEL: &str = indoc! {"
    class Kernel = Object new () (
      public main: platform args: arguments = (
        | sum ::= 0. |
        {1. 2. 3} do: [:each | sum:: sum + each].
        platform transcript show: sum printString; newLine.
      )
    ) : ()
"};

/// The top frame's name, line and source.
fn top_placed(client: &mut DapClient) -> (Value, Value, Value) {
    let frames = client.stack_frames(0, 1);
    (
        frames[0]["name"].clone(),
        frames[0]["line"].clone(),
        frames[0]["source"].clone(),
    )
}

/// A program module named like a shipped one is another module: a
/// breakpoint in the core library's `Kernel.ns` stops only in the shipped
/// `Array>>do:`, shown by the library file's path, and one by the
/// program's file only in the program's `Kernel`, shown by its own.
#[test]
fn a_program_module_named_kernel_never_takes_the_shipped_kernel_s_breakpoints() {
    let (_directory, path) = program("Kernel", OWN_KERNEL);
    let (_library_directory, library) = core_library();
    let shipped = file_source(&library.join("Kernel.ns"));
    let own = file_source(&path);
    let body = array_do_body_line();
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach_with(json!({ "stopOnEntry": false, "coreLibrary": absolute(&library) }));
    let in_shipped = client.set_breakpoints(shipped.clone(), &[body]);
    assert_eq!(in_shipped[0]["verified"], true, "{in_shipped:?}");
    assert_eq!(in_shipped[0]["line"], body, "{in_shipped:?}");
    let in_own = client.set_breakpoints(own.clone(), &[5]);
    assert_eq!(in_own[0]["verified"], true, "{in_own:?}");
    client.configuration_done();

    for element in 1..=3 {
        client.stopped("breakpoint");
        let (name, line, source) = top_placed(&mut client);
        assert_eq!(
            name, "newspeak.core.Kernel/Array>>do:",
            "the stop at element {element}"
        );
        assert_eq!(line, body, "the stop at element {element}");
        assert_eq!(source["path"], shipped["path"], "{source}");
        assert!(source.get("sourceReference").is_none(), "{source}");
        client.succeed("continue", json!({ "threadId": 1 }));
    }
    client.stopped("breakpoint");
    let (name, line, source) = top_placed(&mut client);
    assert_eq!(name, "Kernel>>main:args:");
    assert_eq!(line, 5);
    assert_eq!(source["path"], own["path"], "{source}");
    client.set_breakpoints(shipped, &[]);
    client.set_breakpoints(own, &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "6\n");
}

/// A program whose own root is named `Kernel`, like the shipped module,
/// that signals the shipped `Error`.
const OWN_KERNEL_FAILING: &str = indoc! {"
    class Kernel = Object new () (
      public main: platform args: arguments = (
        Error signal: 'boom'.
      )
    ) : ()
"};

/// A program's own module `Kernel` and the binary's `newspeak.core.Kernel`
/// are named apart: a function breakpoint names each by its id, and one
/// in the form before ids, `Kernel.Array`, is refused with the form that
/// names the class inside the program's `Kernel`, never read as a module
/// of its own.
#[test]
fn a_function_breakpoint_names_the_program_s_kernel_and_the_shipped_one_apart() {
    let (_directory, path) = program("Kernel", OWN_KERNEL);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    let body = client.succeed(
        "setFunctionBreakpoints",
        json!({
            "breakpoints": [
                { "name": "Kernel>>main:args:" },
                { "name": "newspeak.core.Kernel/Array>>do:" },
                { "name": "Kernel.Array>>do:" },
            ],
        }),
    );
    let breakpoints = dap::array(&body["breakpoints"]);
    assert_eq!(breakpoints[0]["verified"], true, "{body}");
    assert_eq!(breakpoints[1]["verified"], true, "{body}");
    assert_eq!(breakpoints[2]["verified"], false, "{body}");
    assert_eq!(
        breakpoints[2]["message"],
        "no module `Kernel.Array`; a class inside a module is written `Kernel/Array`",
        "{body}"
    );
    client.configuration_done();
    let stopped = client.stopped("function breakpoint");
    assert_eq!(
        stopped["body"]["hitBreakpointIds"],
        json!([breakpoints[0]["id"]]),
        "{stopped}"
    );
    assert_eq!(top_frame(&mut client).0, json!("Kernel>>main:args:"));
    client.succeed("continue", json!({ "threadId": 1 }));
    let stopped = client.stopped("function breakpoint");
    assert_eq!(
        stopped["body"]["hitBreakpointIds"],
        json!([breakpoints[1]["id"]]),
        "{stopped}"
    );
    assert_eq!(
        top_frame(&mut client).0,
        json!("newspeak.core.Kernel/Array>>do:")
    );
    client.succeed("setFunctionBreakpoints", json!({ "breakpoints": [] }));
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "6\n");
}

/// A version `version` of the module `com.acme.util.Util`, whose
/// `answer` its nested class answers with `answer`.
fn versioned_util(version: &str, answer: &str) -> String {
    formatdoc! {"
        (* :module: com.acme.util.Util@{version} *)
        class Util = Object new () (
          public answer = (
            ^Inner new answer
          )
          public class Inner = Object new () (
            public answer = (
              ^'{answer}'
            )
          )
        ) : ()
    "}
}

/// A project of two versions of `com.acme.util.Util` and a root that asks
/// for both and prints their answers.
fn two_versions() -> ScratchDirectory {
    let directory = ScratchDirectory::new("two-versions");
    let files = [
        (
            "App.ns",
            indoc! {"
                class App packageUsing: manifest = Object new (
                  |
                  private first = manifest at: 'com.acme.util.Util' version: '1'.
                  private second = manifest at: 'com.acme.util.Util' version: '2'.
                  |
                ) (
                  public main: platform args: arguments = (
                    platform transcript show: first new answer; newLine.
                    platform transcript show: second new answer; newLine.
                  )
                ) : ()
            "}
            .to_string(),
        ),
        ("libs/one/Util.ns", versioned_util("1.0.0", "one")),
        ("libs/two/Util.ns", versioned_util("2.0.0", "two")),
    ];
    for (file, text) in files {
        let path = directory.join(file);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
        std::fs::write(path, text).expect("a module");
    }
    std::fs::write(
        directory.join("newspeak.json"),
        r#"{ "main": "App.ns", "modules": ["libs/one/Util.ns", "libs/two/Util.ns"] }"#,
    )
    .expect("the project file");
    directory
}

/// Each frame of a program of two versions of one module names its
/// module's id and version, a `/` before the class inside it: a function
/// breakpoint without a version stops in each version, and the stack
/// trace tells the two apart.
#[test]
fn the_stack_trace_names_each_version_of_a_module() {
    let directory = two_versions();
    let mut client = DapClient::connect_mode(directory.path());
    client.initialize();
    client.attach(false);
    let body = client.succeed(
        "setFunctionBreakpoints",
        json!({ "breakpoints": [{ "name": "com.acme.util.Util/Inner>>answer" }] }),
    );
    assert_eq!(body["breakpoints"][0]["verified"], true, "{body}");
    client.configuration_done();
    let mut stacks = Vec::new();
    for _ in 0..2 {
        client.stopped("function breakpoint");
        let names: Vec<Value> = client
            .stack_frames(0, 0)
            .iter()
            .map(|frame| frame["name"].clone())
            .collect();
        stacks.push(names);
        client.succeed("setFunctionBreakpoints", json!({ "breakpoints": [] }));
        if stacks.len() == 1 {
            client.succeed(
                "setFunctionBreakpoints",
                json!({ "breakpoints": [{ "name": "com.acme.util.Util@2.0.0/Inner>>answer" }] }),
            );
        }
        client.succeed("continue", json!({ "threadId": 1 }));
    }
    assert_eq!(
        stacks,
        [
            [
                "com.acme.util.Util@1.0.0/Inner>>answer",
                "com.acme.util.Util@1.0.0>>answer",
                "App>>main:args:",
            ],
            [
                "com.acme.util.Util@2.0.0/Inner>>answer",
                "com.acme.util.Util@2.0.0>>answer",
                "App>>main:args:",
            ],
        ]
    );
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "one\ntwo\n");
}

/// An exception filter's class names the shipped module by its id, which
/// no program module shares: `newspeak.core.Kernel/Error` with a program
/// module `Kernel` stops the shipped `Error` signalled.
#[test]
fn an_exception_filter_reaches_the_shipped_class_of_a_shadowed_module_name() {
    let (_directory, path) = program("Kernel", OWN_KERNEL_FAILING);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    client.succeed(
        "setExceptionBreakpoints",
        json!({
            "filters": [],
            "filterOptions": [{
                "filterId": "uncaught",
                "condition": "newspeak.core.Kernel/Error",
            }],
        }),
    );
    client.configuration_done();
    let stopped = client.stopped("exception");
    assert_eq!(stopped["body"]["description"], "Error", "{stopped}");
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 1);
}

/// A session of [`OWN_KERNEL`], a program whose root is named `Kernel`
/// like the shipped module, with the core library taken, the shipped
/// filter off and the step filter entries `entries` answers for the
/// program's file and the library's directory, standing at the `do:`
/// statement on line 4 after a step from the entry; and the `output`
/// events' text the agent sent by then.
struct OwnKernel {
    client: DapClient,
    program: PathBuf,
    library: PathBuf,
    said: String,
    _directories: (ScratchDirectory, ScratchDirectory),
}

fn own_kernel_filtered(entries: impl FnOnce(&Path, &Path) -> Value) -> OwnKernel {
    let (directory, path) = program("Kernel", OWN_KERNEL);
    let (library_directory, library) = core_library();
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach_with(json!({ "stopOnEntry": true, "coreLibrary": absolute(&library) }));
    client.succeed(
        "nsNext/setStepFilters",
        json!({ "shipped": false, "entries": entries(&path, &library) }),
    );
    // Said before the answer, which waits for the VM's.
    let said = client
        .buffered_events()
        .iter()
        .filter(|event| event["event"] == "output")
        .filter_map(|event| event["body"]["output"].as_str())
        .collect::<String>();
    client.configuration_done();
    client.stopped("entry");
    // The entry stands at the local's initializer on line 3.
    client.succeed("stepIn", by_statement());
    client.stopped("step");
    assert_eq!(
        top_frame(&mut client),
        (json!("Kernel>>main:args:"), json!(4))
    );
    OwnKernel {
        client,
        program: path,
        library,
        said,
        _directories: (directory, library_directory),
    }
}

fn by_statement() -> Value {
    json!({ "threadId": 1, "granularity": "statement" })
}

/// A step filter's entry names its module by its file, as a breakpoint
/// does: the library's `Kernel.ns` and the class path `Array` filter the
/// shipped `Array` and not the program's module named `Kernel`, so that
/// a step into the `do:` passes the shipped `Array>>do:` and lands in
/// the program's block.
#[test]
fn a_step_filter_by_the_library_file_filters_only_the_shipped_module() {
    let mut session = own_kernel_filtered(
        |_, library| json!([{ "source": file_source(&library.join("Kernel.ns")), "classPath": ["Array"] }]),
    );
    assert_eq!(session.said, "");
    let client = &mut session.client;
    client.succeed("stepIn", by_statement());
    client.stopped("step");
    assert_eq!(
        top_frame(client),
        (json!("[] in Kernel>>main:args:"), json!(4))
    );
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(client), 0);
}

/// The program's own `Kernel.ns` with no class path filters the whole
/// program module and not the shipped one: a step into the `do:` stops
/// in the shipped `Array>>do:`, shown by the library's file, and a step
/// into from there never lands in the program's block.
#[test]
fn a_step_filter_by_the_program_file_filters_only_the_program_module() {
    let mut session = own_kernel_filtered(
        |program, _| json!([{ "source": file_source(program), "classPath": [] }]),
    );
    let kernel = file_source(&session.library.join("Kernel.ns"));
    let client = &mut session.client;
    client.succeed("stepIn", by_statement());
    client.stopped("step");
    let (name, _, source) = top_placed(client);
    assert_eq!(name, "newspeak.core.Kernel/Array>>do:");
    assert_eq!(source["path"], kernel["path"], "{source}");
    for _ in 0..3 {
        client.succeed("stepIn", by_statement());
        client.stopped("step");
        let (name, _) = top_frame(client);
        assert!(
            !name
                .as_str()
                .unwrap_or_default()
                .contains("in Kernel>>main:args:"),
            "{name}"
        );
    }
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(client), 0);
}

/// The step filters' entries, among them three the agent cannot take: a
/// file of no module of the program, a class its module does not
/// declare and a malformed path.
fn partly_unmappable(program: &Path, library: &Path) -> Value {
    let elsewhere = program.with_file_name("Elsewhere.ns");
    json!([
        { "source": file_source(&elsewhere), "classPath": [] },
        { "source": file_source(&library.join("Kernel.ns")), "classPath": ["NoSuchClass"] },
        { "source": file_source(program), "classPath": [""] },
        { "source": file_source(&library.join("Kernel.ns")), "classPath": ["Array"] },
    ])
}

/// The output events among `events` that say a step filter was left out.
fn left_out_said(events: &[Value]) -> String {
    events
        .iter()
        .filter(|event| event["event"] == "output")
        .filter_map(|event| event["body"]["output"].as_str())
        .filter(|output| output.contains("a step filter is left out"))
        .collect()
}

/// An entry the agent cannot take is left out and the others apply: one
/// whose file is no module of the program silently, since a project's
/// list may name other programs' files, and a class its module does not
/// declare or a malformed path said in an `output` event -- once for as
/// long as the client sends the same filters, and again when they
/// change.
#[test]
fn an_unmappable_step_filter_entry_is_left_out_and_the_others_apply() {
    let mut session = own_kernel_filtered(partly_unmappable);
    let said = session.said.clone();
    assert_eq!(
        said.matches("a step filter is left out").count(),
        2,
        "{said}"
    );
    assert!(!said.contains("Elsewhere.ns"), "{said}");
    assert!(said.contains("declares no class `NoSuchClass`"), "{said}");
    let client = &mut session.client;
    client.succeed("stepIn", by_statement());
    client.stopped("step");
    assert_eq!(
        top_frame(client),
        (json!("[] in Kernel>>main:args:"), json!(4))
    );
    let (program, library) = (session.program.clone(), session.library.clone());
    let entries = partly_unmappable(&program, &library);
    client.succeed(
        "nsNext/setStepFilters",
        json!({ "shipped": false, "entries": entries }),
    );
    assert_eq!(left_out_said(&client.buffered_events()), "", "said once");
    let mut changed = entries.as_array().cloned().unwrap_or_default();
    changed.push(json!({ "source": file_source(&program), "classPath": ["Missing"] }));
    client.succeed(
        "nsNext/setStepFilters",
        json!({ "shipped": false, "entries": changed }),
    );
    let said = left_out_said(&client.buffered_events());
    assert_eq!(
        said.matches("a step filter is left out").count(),
        3,
        "said again once changed: {said}"
    );
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(client), 0);
}

/// A request that still names typed `classes`, the form `entries`
/// replaced, is refused by an error naming the new form, rather than
/// leaving its filters unapplied unsaid.
#[test]
fn a_step_filter_request_with_typed_classes_is_refused() {
    let (_directory, path) = program("Walker", WALKER);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    let response = client.request(
        "nsNext/setStepFilters",
        json!({ "shipped": true, "classes": ["Walker"] }),
    );
    assert_eq!(response["success"], false, "{response}");
    let message = response["message"].as_str().unwrap_or_default();
    assert!(message.contains("entries"), "{response}");
    assert!(message.contains("classPath"), "{response}");
    client.configuration_done();
    assert_eq!(exit_code_reported(&mut client), 0);
}

/// Without a core library, a shipped module has no file the client could
/// name, and its name names none: `Kernel.ns` by name is the program's
/// file-less module of that name or nothing, never the shipped one.
#[test]
fn a_shipped_module_is_never_named_by_its_name() {
    let (_directory, path) = program("Walker", WALKER);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    let breakpoints =
        client.set_breakpoints(json!({ "name": "Kernel.ns" }), &[array_do_body_line()]);
    assert_eq!(breakpoints[0]["verified"], false, "{breakpoints:?}");
    client.configuration_done();
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "12\n");
}

/// A core library whose fingerprint is not the binary's holds texts this
/// binary does not run: the agent says so and does not use it -- its
/// files name no module, and a shipped frame is shown by reference.
#[test]
fn a_core_library_of_another_fingerprint_is_ignored() {
    let (_directory, path) = program("Walker", WALKER);
    let (_library_directory, library) = core_library();
    std::fs::write(library.join("fingerprint"), "0000000000000000\n").expect("another fingerprint");
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    let response = client.request(
        "attach",
        json!({ "stopOnEntry": true, "coreLibrary": absolute(&library) }),
    );
    assert_eq!(response["success"], true, "{response}");
    let output = client.event("output");
    let said = output["body"]["output"].as_str().unwrap_or_default();
    assert!(said.contains("is not used"), "{output}");
    client.event("initialized");
    let kernel = file_source(&library.join("Kernel.ns"));
    let breakpoints = client.set_breakpoints(kernel, &[array_do_body_line()]);
    assert_eq!(breakpoints[0]["verified"], false, "{breakpoints:?}");
    client.succeed(
        "nsNext/setStepFilters",
        json!({ "shipped": false, "entries": [] }),
    );
    client.configuration_done();
    client.stopped("entry");
    client.succeed("stepIn", json!({ "threadId": 1 }));
    client.stopped("step");
    let (name, _, source) = top_placed(&mut client);
    assert_eq!(name, "newspeak.core.Kernel/Array>>do:");
    assert!(source.get("path").is_none(), "{source}");
    assert!(
        source["sourceReference"].as_i64().is_some_and(|id| id > 0),
        "{source}"
    );
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
}

/// A root with a class of its own named `Array`, whose instance holds two
/// slots, made before line 11.
const SHADOW: &str = indoc! {"
    class Shadow = Object new () (
      class Array = (
        |
          first = 1.
          second = 2.
        |
      ) ()
      public main: platform args: arguments = (
        | value |
        value:: Array new.
        platform transcript show: 'done'; newLine.
      )
    ) : ()
"};

/// Whether a value's children are elements is the VM's to say, by the
/// object's shape: a program's own class named `Array` shows its slots as
/// named variables.
#[test]
fn a_program_class_named_array_shows_named_variables() {
    let (_directory, path) = program("Shadow", SHADOW);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[11]);
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 1);
    let locals = locals_of(&mut client, &frames[0]);
    let variables = client.variables(&locals, None, None);
    let value = variable(&variables, "value");
    assert_eq!(value["type"], "Array", "{value}");
    assert!(value.get("indexedVariables").is_none(), "{value}");
    let fields = client.variables(&value["variablesReference"], None, None);
    let names: Vec<&Value> = fields.iter().map(|field| &field["name"]).collect();
    assert_eq!(names, [&json!("first"), &json!("second")]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
}

/// A root with slots and a nested class, which stops on line 8 before it
/// has used its platform's `transcript`.
const NAMED: &str = indoc! {"
    class Named = Object new (
      | n ::= 3. sum ::= 0. |
    ) (
      class KindA = Object new () ()
      public main: platform args: arguments = (
        KindA new.
        sum:: n + 1.
        platform transcript show: sum printString; newLine.
      )
    ) : ()
"};

/// The `name` and `value` of each of `fields`.
fn names_and_values(fields: &[Value]) -> Vec<(String, String)> {
    fields
        .iter()
        .map(|field| {
            (
                field["name"].as_str().unwrap_or_default().to_owned(),
                field["value"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// An instance's fields are named by its slots and nested classes, and a
/// lazy slot its program has not used yet says it is not computed, rather
/// than showing `nil`.
#[test]
fn an_instance_s_fields_are_named_and_a_lazy_slot_not_computed_says_so() {
    let (_directory, path) = program("Named", NAMED);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[8]);
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 1);
    let locals = locals_of(&mut client, &frames[0]);
    let variables = client.variables(&locals, None, None);
    let receiver = variable(&variables, "self");
    let fields = client.variables(&receiver["variablesReference"], None, None);
    assert_eq!(
        names_and_values(&fields),
        [
            ("n".to_owned(), "3".to_owned()),
            ("sum".to_owned(), "4".to_owned()),
            ("KindA".to_owned(), "the class KindA".to_owned()),
        ]
    );
    let platform = variable(&variables, "platform");
    let facets = client.variables(&platform["variablesReference"], None, None);
    let transcript = variable(&facets, "transcript");
    assert_eq!(transcript["value"], "<not computed yet>", "{transcript}");
    assert_eq!(transcript["variablesReference"], 0, "{transcript}");
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "4\n");
}

/// Slot names are structure, not source text: an image built without its
/// source still names an instance's fields.
#[test]
fn a_stripped_image_names_an_instance_s_fields() {
    let (directory, path) = program("Named", NAMED);
    let image = directory.join("named.nsi");
    let (built, _, error) = common::ns_next(&[
        "build".as_ref(),
        path.as_os_str(),
        image.as_os_str(),
        OsStr::new("--strip-source"),
    ]);
    assert!(built, "{error}");
    let mut client = DapClient::connect_mode_of("load", &image);
    client.start_session(true);
    client.stopped("entry");
    let frames = client.stack_frames(0, 0);
    let main = frames
        .iter()
        .find(|frame| frame["name"] == "Named>>main:args:")
        .unwrap_or_else(|| panic!("no frame of `main:args:`: {frames:?}"));
    let locals = locals_of(&mut client, main);
    let variables = client.variables(&locals, None, None);
    let receiver = variable(&variables, "self");
    let fields = client.variables(&receiver["variablesReference"], None, None);
    let names: Vec<&Value> = fields.iter().map(|field| &field["name"]).collect();
    assert_eq!(names, [&json!("n"), &json!("sum"), &json!("KindA")]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
}

/// A program that says it spins, then counts forever on line 7.
const SPINNER: &str = indoc! {"
    class Spinner = Object new () (
      public main: platform args: arguments = (
        | count |
        count:: 0.
        platform transcript show: 'spinning'; newLine.
        [true] whileTrue: [
          count:: count + 1.
        ].
      )
    ) : ()
"};

/// Ends a session whose program spins forever.
fn terminate_the_spinner(mut client: DapClient) {
    client.succeed("terminate", json!({}));
    assert_eq!(exit_code_reported(&mut client), 1);
    assert_eq!(client.finish().status, Some(1));
}

/// The top frame's name and line.
fn top_frame(client: &mut DapClient) -> (Value, Value) {
    let frames = client.stack_frames(0, 1);
    (frames[0]["name"].clone(), frames[0]["line"].clone())
}

#[test]
fn a_breakpoint_set_while_running_stops_the_loop() {
    let (_directory, path) = program("Spinner", SPINNER);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(false);
    client.wait_for_stdout("spinning\n");
    let breakpoints = client.set_breakpoints(file_source(&path), &[7]);
    assert_eq!(breakpoints[0]["verified"], true, "{breakpoints:?}");
    let stopped = client.stopped("breakpoint");
    assert_eq!(
        stopped["body"]["hitBreakpointIds"],
        json!([breakpoints[0]["id"]]),
        "{stopped}"
    );
    assert_eq!(top_frame(&mut client).1, 7);
    terminate_the_spinner(client);
}

/// A root whose line 10 sends `do:` with a block that sends `sink:`.
const BLOCKS: &str = indoc! {"
    class Blocks = Object new (
      |
        total ::= 0.
      |
    ) (
      sink: value = (
        total:: total + value.
      )
      public main: platform args: arguments = (
        {1. 2} do: [:each | sink: each].
        platform transcript show: total printString; newLine.
      )
    ) : ()
"};

#[test]
fn a_block_variant_stops_inside_the_block_only() {
    let (_directory, path) = program("Blocks", BLOCKS);
    let mut client = DapClient::connect_mode(&path);
    let line = breakpoints_before_the_start(&mut client, &path, &[10]);
    assert_eq!(line[0]["verified"], true, "{line:?}");
    client.stopped("breakpoint");
    assert_eq!(
        top_frame(&mut client),
        (json!("Blocks>>main:args:"), json!(10))
    );

    let text = BLOCKS.lines().nth(9).expect("line 10");
    let column = text.find("sink:").expect("the block's send") + 1;
    let body = client.succeed(
        "setBreakpoints",
        json!({
            "source": file_source(&path),
            "breakpoints": [{ "line": 10, "column": column }],
        }),
    );
    let variant = &body["breakpoints"][0];
    assert_eq!(variant["verified"], true, "{body}");
    assert_eq!(variant["column"], column, "{body}");
    for _ in 0..2 {
        client.succeed("continue", json!({ "threadId": 1 }));
        let stopped = client.stopped("breakpoint");
        assert_eq!(
            stopped["body"]["hitBreakpointIds"],
            json!([variant["id"]]),
            "{stopped}"
        );
        assert_eq!(
            top_frame(&mut client),
            (json!("[] in Blocks>>main:args:"), json!(10))
        );
    }
    client.set_breakpoints(file_source(&path), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "3\n");
}

#[test]
fn a_pass_count_stops_at_that_iteration() {
    let (_directory, path) = program("Loop", LOOP);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    let body = client.succeed(
        "setBreakpoints",
        json!({
            "source": file_source(&path),
            "breakpoints": [
                { "line": 6, "hitCondition": "3" },
                { "line": 8, "hitCondition": "3 times" },
            ],
        }),
    );
    let breakpoints = dap::array(&body["breakpoints"]);
    assert_eq!(breakpoints[0]["verified"], true, "{body}");
    assert_eq!(breakpoints[1]["verified"], false, "{body}");
    assert!(
        breakpoints[1]["message"]
            .as_str()
            .is_some_and(|message| message.contains("3 times")),
        "{body}"
    );
    client.configuration_done();
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 0);
    let block = &frames[0];
    assert_eq!(block["line"], 6, "{block}");
    let locals = locals_of(&mut client, block);
    let variables = client.variables(&locals, None, None);
    assert_eq!(variable(&variables, "index")["value"], "3", "{variables:?}");
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "15\n");
}

/// A root whose line 4 is a comment above the statement on line 5.
const COMMENTED: &str = indoc! {"
    class Commented = Object new () (
      public main: platform args: arguments = (
        | count |
        /// The count starts at 3.
        count:: 3.
        platform transcript show: count printString; newLine.
      )
    ) : ()
"};

#[test]
fn a_breakpoint_on_a_comment_line_is_moved_to_the_next_statement() {
    let (_directory, path) = program("Commented", COMMENTED);
    let mut client = DapClient::connect_mode(&path);
    let breakpoints = breakpoints_before_the_start(&mut client, &path, &[4]);
    assert_eq!(breakpoints[0]["verified"], true, "{breakpoints:?}");
    assert_eq!(breakpoints[0]["line"], 5, "{breakpoints:?}");
    client.stopped("breakpoint");
    assert_eq!(top_frame(&mut client).1, 5);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
}

/// A root whose line 4 holds two statements.
const LINES: &str = indoc! {"
    class Lines = Object new () (
      public main: platform args: arguments = (
        | first second |
        first:: 1. second:: 2.
        platform transcript show: (first + second) printString; newLine.
      )
    ) : ()
"};

/// The column of `text` on line 4 of `LINES`, counted from 1.
fn column_in_lines(text: &str) -> usize {
    let line = LINES.lines().nth(3).expect("line 4");
    line.find(text).expect("a statement") + 1
}

/// One `next` from the start of a line of two statements stops on the
/// next line, past the line's second statement.
#[test]
fn next_by_line_steps_over_a_whole_line() {
    let (_directory, path) = program("Lines", LINES);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(true);
    client.stopped("entry");
    let frames = client.stack_frames(0, 1);
    assert_eq!(frames[0]["line"], 4, "{frames:?}");
    assert_eq!(
        frames[0]["column"],
        column_in_lines("first::"),
        "{frames:?}"
    );

    client.succeed("next", json!({ "threadId": 1 }));
    client.stopped("step");
    let frames = client.stack_frames(0, 0);
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0]["line"], 5, "{frames:?}");
    let locals = locals_of(&mut client, &frames[0]);
    let variables = client.variables(&locals, None, None);
    assert_eq!(
        variable(&variables, "second")["value"],
        "2",
        "{variables:?}"
    );
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "3\n");
}

#[test]
fn next_by_statement_stops_mid_line() {
    let (_directory, path) = program("Lines", LINES);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(true);
    client.stopped("entry");
    client.succeed("next", json!({ "threadId": 1, "granularity": "statement" }));
    client.stopped("step");
    let frames = client.stack_frames(0, 1);
    assert_eq!(frames[0]["line"], 4, "{frames:?}");
    assert_eq!(
        frames[0]["column"],
        column_in_lines("second::"),
        "{frames:?}"
    );
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
}

/// A root whose line 10 is a whole loop over a block that sends `sink:`.
const ONE_LINE_LOOP: &str = indoc! {"
    class OneLine = Object new (
      |
        total ::= 0.
      |
    ) (
      sink: value = (
        total:: total + value.
      )
      public main: platform args: arguments = (
        1 to: 3 do: [:index | sink: index].
        platform transcript show: total printString; newLine.
      )
    ) : ()
"};

#[test]
fn a_line_step_over_a_one_line_loop_ends() {
    let (_directory, path) = program("OneLine", ONE_LINE_LOOP);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(true);
    client.stopped("entry");
    assert_eq!(
        top_frame(&mut client),
        (json!("OneLine>>main:args:"), json!(10))
    );
    client.succeed("next", json!({ "threadId": 1 }));
    client.stopped("step");
    assert_eq!(
        top_frame(&mut client),
        (json!("OneLine>>main:args:"), json!(11))
    );
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "6\n");
}

/// A root whose line 5 is a `do:` over a one-line block, followed on line
/// 6 by the statement after it.
const ONE_LINE_BLOCK: &str = indoc! {"
    class OneLineBlock = Object new (
      | sum ::= 0. |
    ) (
      public main: platform args: arguments = (
        {1. 2. 3} do: [:each | sum:: sum + each].
        platform transcript show: sum printString; newLine.
      )
    ) : ()
"};

/// The value of `name` among the top frame's locals.
fn top_local(client: &mut DapClient, name: &str) -> Value {
    let frames = client.stack_frames(0, 1);
    let locals = locals_of(client, &frames[0]);
    let variables = client.variables(&locals, None, None);
    variable(&variables, name)["value"].clone()
}

/// A `next` by line at a one-line block's statement stops in the block's
/// next activation, though it stands on the same line in a frame of the
/// same name and depth: a new activation is another place.
#[test]
fn a_line_step_over_at_a_one_line_block_stops_in_its_next_activation() {
    let (_directory, path) = program("OneLineBlock", ONE_LINE_BLOCK);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(true);
    client.stopped("entry");
    client.succeed("stepIn", json!({ "threadId": 1 }));
    client.stopped("step");
    assert_eq!(
        top_frame(&mut client),
        (json!("[] in OneLineBlock>>main:args:"), json!(5))
    );
    assert_eq!(top_local(&mut client, "each"), "1");

    client.succeed("next", json!({ "threadId": 1 }));
    client.stopped("step");
    assert_eq!(
        top_frame(&mut client),
        (json!("[] in OneLineBlock>>main:args:"), json!(5))
    );
    assert_eq!(top_local(&mut client, "each"), "2");
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "6\n");
}

/// A root whose line 6 sends `ifTrue:` with a one-line block to an object
/// that runs it twice, from its lines 11 and 12: the block the compiler
/// inlines runs as its fallback closure, twice at one depth.
const FALLING_TWICE: &str = indoc! {"
    class FallingTwice = Object new (
      | count ::= 0. twice = Twice new. |
    ) (
      public main: platform args: arguments = (
        | before |
        twice ifTrue: [count:: count + 10].
        platform transcript show: count printString; newLine.
      )
      public class Twice = Object new () (
        public ifTrue: block = (
          block value.
          ^block value
        )
      )
    ) : ()
"};

/// A `next` by line at a one-line fallback block's statement stops in the
/// block's next activation, which its receiver's `ifTrue:` runs from its
/// next line: a new activation is another place, though at the same
/// depth, in a frame of the same name, on the same line.
#[test]
fn a_line_step_over_at_a_fallback_block_stops_in_its_next_activation() {
    let (_directory, path) = program("FallingTwice", FALLING_TWICE);
    let text = FALLING_TWICE.lines().nth(5).expect("line 6");
    let column = text
        .find("count:: count + 10")
        .expect("the block's statement")
        + 1;
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    let body = client.succeed(
        "setBreakpoints",
        json!({
            "source": file_source(&path),
            "breakpoints": [{ "line": 6, "column": column }],
        }),
    );
    assert_eq!(body["breakpoints"][0]["verified"], true, "{body}");
    client.configuration_done();
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 2);
    assert_eq!(
        frames[0]["name"], "[] in FallingTwice>>main:args:",
        "{frames:?}"
    );
    assert_eq!(frames[0]["line"], 6, "{frames:?}");
    assert_eq!(frames[1]["line"], 11, "{frames:?}");
    client.succeed(
        "setBreakpoints",
        json!({ "source": file_source(&path), "breakpoints": [] }),
    );

    client.succeed("next", json!({ "threadId": 1 }));
    client.stopped("step");
    let frames = client.stack_frames(0, 2);
    assert_eq!(
        frames[0]["name"], "[] in FallingTwice>>main:args:",
        "{frames:?}"
    );
    assert_eq!(frames[0]["line"], 6, "{frames:?}");
    assert_eq!(
        frames[1]["line"], 12,
        "the block's second activation: {frames:?}"
    );
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "20\n");
}

/// A root whose line 4 halts and then runs two more statements, followed
/// on line 5 by the statement after them.
const HALTING: &str = indoc! {"
    class Halting = Object new () (
      public main: platform args: arguments = (
        | first second |
        self halt. first:: 1. second:: 2.
        platform transcript show: (first + second) printString; newLine.
      )
    ) : ()
"};

/// A `next` by line from a `halt`'s stop, which stands in the shown frame
/// below the exception's machinery, steps over the rest of the line in
/// that frame and stops on the next line.
#[test]
fn a_line_step_from_a_halt_stop_leaves_the_line() {
    let (_directory, path) = program("Halting", HALTING);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    client.succeed("setExceptionBreakpoints", json!({ "filters": ["halt"] }));
    client.configuration_done();
    let stopped = client.stopped("exception");
    assert_eq!(stopped["body"]["text"], "halt", "{stopped}");
    client.succeed("next", json!({ "threadId": 1 }));
    client.stopped("step");
    let frames = client.stack_frames(0, 0);
    let shown: Vec<&Value> = frames
        .iter()
        .filter(|frame| frame.get("presentationHint").is_none())
        .collect();
    assert_eq!(shown[0]["name"], "Halting>>main:args:", "{frames:?}");
    assert_eq!(shown[0]["line"], 5, "{frames:?}");
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "3\n");
}

/// A root whose line 4 sends `double:`, a method of its own.
const CALLS: &str = indoc! {"
    class Calls = Object new () (
      public main: platform args: arguments = (
        | value |
        value:: self double: 21.
        platform transcript show: value printString; newLine.
      )
      double: number = (
        ^number * 2
      )
    ) : ()
"};

#[test]
fn step_in_and_out_stop_in_the_callee_and_back() {
    let (_directory, path) = program("Calls", CALLS);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(true);
    client.stopped("entry");
    client.succeed("stepIn", json!({ "threadId": 1 }));
    client.stopped("step");
    let frames = client.stack_frames(0, 0);
    assert_eq!(frames.len(), 2, "{frames:?}");
    assert_eq!(frames[0]["name"], "Calls>>double:", "{frames:?}");
    assert_eq!(frames[0]["line"], 8, "{frames:?}");

    client.succeed("stepOut", json!({ "threadId": 1 }));
    client.stopped("step");
    let frames = client.stack_frames(0, 0);
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0]["name"], "Calls>>main:args:", "{frames:?}");
    assert_eq!(frames[0]["line"], 4, "{frames:?}");
    let scopes = client.succeed("scopes", json!({ "frameId": 1 }));
    let returned = &scopes["scopes"][1];
    assert_eq!(returned["name"], "Return value", "{scopes}");
    assert_eq!(returned["presentationHint"], "returnValue", "{scopes}");
    let value = client.variables(&returned["variablesReference"], None, None);
    assert_eq!(value[0]["value"], "42", "{value:?}");
    // Named by the send in the caller that the callee returned to.
    assert_eq!(value[0]["name"], "self double: 21", "{value:?}");

    client.succeed("next", json!({ "threadId": 1 }));
    client.stopped("step");
    assert_eq!(
        top_frame(&mut client),
        (json!("Calls>>main:args:"), json!(5))
    );
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "42\n");
}

/// A root whose line 14 sends `value` to an element of an `Array` of
/// two instances: a `KindA`, which declares no slot, and a `Holder`,
/// which declares one.
const KINDS: &str = indoc! {"
    class Kinds = Object new () (
      class KindA = Object new () (
        public value = (
          ^1
        )
      )
      class Holder = Object new (
        | public contents ::= 5. |
      ) ()
      public main: platform args: arguments = (
        | kinds which total |
        kinds:: {KindA new. Holder new}.
        which:: 1.
        total:: (kinds at: which) value.
        platform transcript show: total printString; newLine.
      )
    ) : ()
"};

/// A step over a line reports the value of its last call in the scope
/// "Return value", the one variable there named by the call's source.
#[test]
fn a_step_over_names_the_return_value_by_its_call() {
    let (_directory, path) = program("Kinds", KINDS);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[14]);
    client.stopped("breakpoint");
    client.succeed("next", json!({ "threadId": 1 }));
    client.stopped("step");
    assert_eq!(
        top_frame(&mut client),
        (json!("Kinds>>main:args:"), json!(15))
    );
    let scopes = client.succeed("scopes", json!({ "frameId": 1 }));
    let returned = &scopes["scopes"][1];
    assert_eq!(returned["name"], "Return value", "{scopes}");
    assert_eq!(returned["presentationHint"], "returnValue", "{scopes}");
    let value = client.variables(&returned["variablesReference"], None, None);
    assert_eq!(value.len(), 1, "{value:?}");
    assert_eq!(value[0]["name"], "(kinds at: which) value", "{value:?}");
    assert_eq!(value[0]["value"], "1", "{value:?}");
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "1\n");
}

/// An element is expandable exactly when it has fields: a `KindA`, which
/// declares no slot, has no children and so no reference, and a
/// `Holder` one, its slot.
#[test]
fn only_an_element_with_fields_expands() {
    let (_directory, path) = program("Kinds", KINDS);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[14]);
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 1);
    let locals = locals_of(&mut client, &frames[0]);
    let variables = client.variables(&locals, None, None);
    let kinds = variable(&variables, "kinds");
    assert_eq!(kinds["indexedVariables"], 2, "{kinds}");
    let elements = client.variables(&kinds["variablesReference"], None, None);
    let kind = variable(&elements, "[1]");
    assert_eq!(kind["type"], "KindA", "{kind}");
    assert_eq!(kind["variablesReference"], 0, "{kind}");
    let holder = variable(&elements, "[2]");
    assert_eq!(holder["type"], "Holder", "{holder}");
    assert_ne!(holder["variablesReference"], 0, "{holder}");
    let fields = client.variables(&holder["variablesReference"], None, None);
    let fields: Vec<(Value, Value)> = fields
        .iter()
        .map(|field| (field["name"].clone(), field["value"].clone()))
        .collect();
    assert_eq!(fields, [(json!("contents"), json!("5"))]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
}

/// In an image built without its source, where no body has metadata, a
/// step out names the value it reports by the selector of the send in
/// the caller that the callee returned to.
#[test]
fn a_stripped_image_names_the_return_value_by_its_selector() {
    let (directory, path) = program("Calls", CALLS);
    let image = directory.join("calls.nsi");
    let (built, _, error) = common::ns_next(&[
        "build".as_ref(),
        path.as_os_str(),
        image.as_os_str(),
        OsStr::new("--strip-source"),
    ]);
    assert!(built, "{error}");
    let mut client = DapClient::connect_mode_of("load", &image);
    client.initialize();
    client.attach(false);
    client.succeed(
        "setFunctionBreakpoints",
        json!({ "breakpoints": [{ "name": "Calls>>double:" }] }),
    );
    client.configuration_done();
    client.stopped("function breakpoint");
    assert_eq!(top_frame(&mut client).0, json!("Calls>>double:"));
    client.succeed("setFunctionBreakpoints", json!({ "breakpoints": [] }));
    client.succeed("stepOut", json!({ "threadId": 1 }));
    client.stopped("step");
    assert_eq!(top_frame(&mut client).0, json!("Calls>>main:args:"));
    let scopes = client.succeed("scopes", json!({ "frameId": 1 }));
    let returned = &scopes["scopes"][1];
    assert_eq!(returned["name"], "Return value", "{scopes}");
    let value = client.variables(&returned["variablesReference"], None, None);
    assert_eq!(value[0]["name"], "double:", "{value:?}");
    assert_eq!(value[0]["value"], "42", "{value:?}");
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "42\n");
}

/// A subclass whose superclass's slot initializer sends a method, and
/// whose method sends its superclass's by `super`.
const SUPERS: &str = indoc! {"
    class Supers = Object new () (
      class Base = Object new (
        | public base ::= self seed. |
      ) (
        public seed = (
          ^20
        )
        public value = (
          ^41
        )
      )
      class Derived = Base new (
        | public derived ::= 1. |
      ) (
        public value = (
          ^super value + derived
        )
      )
      public main: platform args: arguments = (
        | made |
        made:: Derived new.
        platform transcript show: (made value + made base) printString; newLine.
      )
    ) : ()
"};

/// The name and the value of the stop's return value, `None` when the
/// stop reports none.
fn returned_of(client: &mut DapClient) -> Option<(Value, Value)> {
    let scopes = client.succeed("scopes", json!({ "frameId": 1 }));
    let returned = scopes["scopes"]
        .as_array()
        .and_then(|scopes| scopes.iter().find(|scope| scope["name"] == "Return value"))?
        .clone();
    let value = client.variables(&returned["variablesReference"], None, None);
    Some((value[0]["name"].clone(), value[0]["value"].clone()))
}

/// In an image built without its source, a step out names the value by
/// the kind of call the caller stands at: `super` before a super send's
/// selector, and the superclass initializer a subclass's level runs.
#[test]
fn a_stripped_image_names_a_super_send_and_a_superclass_initializer() {
    let (directory, path) = program("Supers", SUPERS);
    let image = directory.join("supers.nsi");
    let (built, _, error) = common::ns_next(&[
        "build".as_ref(),
        path.as_os_str(),
        image.as_os_str(),
        OsStr::new("--strip-source"),
    ]);
    assert!(built, "{error}");
    let mut client = DapClient::connect_mode_of("load", &image);
    client.initialize();
    client.attach(false);
    client.succeed(
        "setFunctionBreakpoints",
        json!({ "breakpoints": [{ "name": "Supers/Base>>seed" }] }),
    );
    client.configuration_done();
    client.stopped("function breakpoint");
    // A method breakpoint stops at the method's exit too: each is
    // replaced before the step out that leaves its method.
    client.succeed(
        "setFunctionBreakpoints",
        json!({ "breakpoints": [{ "name": "Supers/Base>>value" }] }),
    );
    let mut stops = Vec::new();
    for _ in 0..2 {
        client.succeed("stepOut", json!({ "threadId": 1 }));
        client.stopped("step");
        stops.push((top_frame(&mut client).0, returned_of(&mut client)));
    }
    client.succeed("continue", json!({ "threadId": 1 }));
    client.stopped("function breakpoint");
    client.succeed("setFunctionBreakpoints", json!({ "breakpoints": [] }));
    client.succeed("stepOut", json!({ "threadId": 1 }));
    client.stopped("step");
    stops.push((top_frame(&mut client).0, returned_of(&mut client)));
    let named =
        |frame: &str, call: &str, value: &str| (json!(frame), Some((json!(call), json!(value))));
    assert_eq!(
        stops,
        [
            named("Supers/Base initializer", "seed", "20"),
            named(
                "Supers/Derived initializer",
                "superclass initializer",
                "an instance of Derived"
            ),
            named("Supers/Derived>>value", "super value", "41"),
        ]
    );
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(
        client.finish().stdout,
        "62
"
    );
}

/// A root whose slot initializers, line 4 the last, run in its
/// instantiation, a host run before the one of `main:args:`, whose first
/// statement is line 8.
const INITIALIZED: &str = indoc! {"
    class Initialized = Object new (
      |
        first ::= 1.
        second ::= first + 1.
      |
    ) (
      public main: platform args: arguments = (
        platform transcript show: second printString; newLine.
      )
    ) : ()
"};

/// One `next` from a breakpoint in the root's last slot initializer goes
/// on past the root's factory, which no step stops in, and out of
/// the root's instantiation into `main:args:`, a host run of its own,
/// where it stops at the first statement for the step.
#[test]
fn next_from_a_slot_initializer_reaches_main() {
    let (_directory, path) = program("Initialized", INITIALIZED);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[4]);
    client.stopped("breakpoint");
    assert_eq!(top_frame(&mut client).1, json!(4));
    client.succeed("next", json!({ "threadId": 1 }));
    client.stopped("step");
    assert_eq!(
        top_frame(&mut client),
        (json!("Initialized>>main:args:"), json!(8))
    );
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "2\n");
}

/// A root whose `main:args:` instantiates a nested class with a slot
/// initializer, then prints the slot's value.
const PARTS: &str = indoc! {"
    class Parts = Object new () (
      class Part = Object new (
        | public value ::= 3. |
      ) ()
      public main: platform args: arguments = (
        | made |
        made:: Part new.
        platform transcript show: made value printString; newLine.
      )
    ) : ()
"};

/// A step into stops in a factory in an image built without its source
/// as well, where no body has metadata: the step into from `main:args:`
/// at `Part new` stops in `Part class>>new`, a frame without a line, and
/// the next step into goes on to `Part`'s initializer.
#[test]
fn a_step_into_stops_in_a_stripped_image_s_factory() {
    let (directory, path) = program("Parts", PARTS);
    let image = directory.join("parts.nsi");
    let (built, _, error) = common::ns_next(&[
        "build".as_ref(),
        path.as_os_str(),
        image.as_os_str(),
        OsStr::new("--strip-source"),
    ]);
    assert!(built, "{error}");
    let mut client = DapClient::connect_mode_of("load", &image);
    client.start_session(true);
    client.stopped("entry");
    // A body without metadata stops at its first safepoint: the entry
    // stands in `Part`'s superclass clause, which `Part new` runs first,
    // and a step into from there stops back in `main:args:`.
    let mut stops = Vec::new();
    let mut factory_lines = Vec::new();
    for _ in 0..3 {
        client.succeed("stepIn", json!({ "threadId": 1 }));
        client.stopped("step");
        let frames = client.stack_frames(0, 0);
        let names: Vec<String> = frames
            .iter()
            .map(|frame| frame["name"].as_str().unwrap_or_default().to_owned())
            .collect();
        factory_lines.extend(
            frames
                .iter()
                .filter(|frame| frame["name"] == "Parts/Part class>>new")
                .map(|frame| frame["line"].clone()),
        );
        stops.push(names);
    }
    assert_eq!(
        stops,
        [
            vec!["Parts>>main:args:"],
            vec!["Parts/Part class>>new", "Parts>>main:args:"],
            vec![
                "Parts/Part initializer",
                "Parts/Part class>>new",
                "Parts>>main:args:"
            ],
        ]
    );
    assert_eq!(factory_lines, [json!(0), json!(0)]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(
        client.finish().stdout,
        "3
"
    );
}

/// A root whose `main:args:` instantiates a nested class whose superclass
/// clause names another nested class with a slot initializer.
const DERIVED_PARTS: &str = indoc! {"
    class DerivedParts = Object new () (
      class Base = Object new (
        | public base ::= 4. |
      ) ()
      class Part = Base new (
        | public value ::= 3. |
      ) ()
      public main: platform args: arguments = (
        | made |
        made:: Part new.
        platform transcript show: (made value + made base) printString; newLine.
      )
    ) : ()
"};

/// A step into a superclass clause stops at the superclass's header in an
/// image built without its source as well: a frame without a line, at the
/// superclass's initializer, which the next step into goes on from as from
/// any push without metadata, back to the subclass's initializer once it
/// returns. The steps from the entry go through the nested classes'
/// superclass clauses first, which `Part new` evaluates before it sends
/// the factory.
#[test]
fn a_step_into_stops_at_a_stripped_image_s_superclass_header() {
    let (directory, path) = program("DerivedParts", DERIVED_PARTS);
    let image = directory.join("derived.nsi");
    let (built, _, error) = common::ns_next(&[
        "build".as_ref(),
        path.as_os_str(),
        image.as_os_str(),
        OsStr::new("--strip-source"),
    ]);
    assert!(built, "{error}");
    let mut client = DapClient::connect_mode_of("load", &image);
    client.start_session(true);
    client.stopped("entry");
    let mut stops = Vec::new();
    let mut header_lines = Vec::new();
    for _ in 0..7 {
        client.succeed("stepIn", json!({ "threadId": 1 }));
        client.stopped("step");
        let frames = client.stack_frames(0, 0);
        let names: Vec<String> = frames
            .iter()
            .map(|frame| frame["name"].as_str().unwrap_or_default().to_owned())
            .collect();
        header_lines.extend(
            frames
                .iter()
                .take(1)
                .filter(|frame| frame["name"] == "DerivedParts/Base initializer")
                .map(|frame| frame["line"].clone()),
        );
        stops.push(names);
    }
    assert_eq!(
        stops,
        [
            vec![
                "DerivedParts/Base superclass clause",
                "DerivedParts/Part superclass clause",
                "DerivedParts>>main:args:"
            ],
            vec![
                "DerivedParts/Part superclass clause",
                "DerivedParts>>main:args:"
            ],
            vec!["DerivedParts>>main:args:"],
            vec!["DerivedParts/Part class>>new", "DerivedParts>>main:args:"],
            vec![
                "DerivedParts/Part initializer",
                "DerivedParts/Part class>>new",
                "DerivedParts>>main:args:"
            ],
            vec![
                "DerivedParts/Base initializer",
                "DerivedParts/Part initializer",
                "DerivedParts/Part class>>new",
                "DerivedParts>>main:args:"
            ],
            vec![
                "DerivedParts/Part initializer",
                "DerivedParts/Part class>>new",
                "DerivedParts>>main:args:"
            ],
        ]
    );
    assert_eq!(header_lines, [json!(0)]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(
        client.finish().stdout,
        "7
"
    );
}

/// A root whose `main:args:` instantiates `Punkt`, whose header names a
/// factory pattern and a superclass clause, itself a class with a factory
/// pattern. The header patterns are on lines 2 and 5.
const POINTS: &str = indoc! {"
    class Points = Object new () (
      class Basis new: a = (
        | public base ::= a. |
      ) ()
      class Punkt x: ax y: ay = Basis new: ax (
        | public y ::= ay. |
      ) ()
      public main: platform args: arguments = (
        | made |
        made:: Punkt x: 1 y: 2.
        platform transcript show: (made y + made base) printString; newLine.
      )
    ) : ()
"};

/// A frame's name, line, column and end of its range.
fn placed(frame: &Value) -> (String, i64, i64, i64, i64) {
    let number = |key: &str| frame[key].as_i64().unwrap_or(-1);
    (
        frame["name"].as_str().unwrap_or_default().to_owned(),
        number("line"),
        number("column"),
        number("endLine"),
        number("endColumn"),
    )
}

/// A step stop whose top frame is answered, after `request`.
fn step_to_top(client: &mut DapClient, request: &str) -> Vec<Value> {
    client.succeed(request, json!({ "threadId": 1 }));
    client.stopped("step");
    client.stack_frames(0, 0)
}

/// The header stops through the agent: a step into the instantiation stops
/// in the factory, named as the stack trace names a factory and placed at
/// the header's factory pattern, whose parameters are the frame's
/// variables; a step over reaches the superclass clause, a step into the
/// clause the superclass's header, and a step out goes back after the clause.
#[test]
fn the_header_stops_reach_a_client_as_frames_at_the_header_pattern() {
    let (_directory, path) = program("Points", POINTS);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(true);
    client.stopped("entry");
    // The entry stands at `main:args:`'s statement. Reading `Punkt` for the
    // first time evaluates its superclass clause, `Basis`, which the first step
    // into enters; the second returns to the statement, at the instantiation.
    step_to_top(&mut client, "stepIn");
    let frames = step_to_top(&mut client, "stepIn");
    assert_eq!(
        placed(&frames[0]),
        ("Points>>main:args:".into(), 10, 5, 10, 27)
    );

    let frames = step_to_top(&mut client, "stepIn");
    assert_eq!(
        frames.iter().map(placed).collect::<Vec<_>>(),
        [
            ("Points/Punkt class>>x:y:".to_owned(), 5, 15, 5, 26),
            ("Points>>main:args:".to_owned(), 10, 18, 10, 25),
        ]
    );
    assert_eq!(frames[0]["source"]["path"], absolute(&path), "{frames:?}");
    let locals = locals_of(&mut client, &frames[0]);
    let variables = client.variables(&locals, None, None);
    assert_eq!(variable(&variables, "ax")["value"], "1", "{variables:?}");
    assert_eq!(variable(&variables, "ay")["value"], "2", "{variables:?}");

    let frames = step_to_top(&mut client, "next");
    assert_eq!(
        placed(&frames[0]),
        ("Points/Punkt initializer".into(), 5, 29, 5, 42)
    );
    assert_eq!(frames.len(), 3, "{frames:?}");

    let frames = step_to_top(&mut client, "stepIn");
    assert_eq!(
        placed(&frames[0]),
        ("Points/Basis initializer".into(), 2, 15, 2, 21)
    );
    assert_eq!(frames.len(), 4, "{frames:?}");
    let locals = locals_of(&mut client, &frames[0]);
    let variables = client.variables(&locals, None, None);
    assert_eq!(variable(&variables, "a")["value"], "1", "{variables:?}");

    let frames = step_to_top(&mut client, "stepOut");
    assert_eq!(
        placed(&frames[0]),
        ("Points/Punkt initializer".into(), 6, 7, 6, 23)
    );
    assert_eq!(frames.len(), 3, "{frames:?}");
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(
        client.finish().stdout,
        "3
"
    );
}

/// A session of `POINTS` stopped by steps into at `Punkt`'s factory, which
/// a step into from the sender reaches by undoing the initializer's push.
fn at_the_factory_stop(path: &Path) -> DapClient {
    let mut client = DapClient::connect_mode(path);
    client.start_session(true);
    client.stopped("entry");
    step_to_top(&mut client, "stepIn");
    step_to_top(&mut client, "stepIn");
    let frames = step_to_top(&mut client, "stepIn");
    assert_eq!(frames[0]["name"], "Points/Punkt class>>x:y:", "{frames:?}");
    client
}

/// A `continue` at the factory's stop runs the program to its end, the
/// initializer the factory pushes again run once.
#[test]
fn a_continue_at_the_factory_stop_runs_the_program_to_its_end() {
    let (_directory, path) = program("Points", POINTS);
    let mut client = at_the_factory_stop(&path);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "3\n");
}

/// A `terminate` at the factory's stop ends the program there.
#[test]
fn a_terminate_at_the_factory_stop_ends_the_program() {
    let (_directory, path) = program("Points", POINTS);
    let mut client = at_the_factory_stop(&path);
    client.succeed("terminate", json!({}));
    assert_eq!(exit_code_reported(&mut client), 1);
    let finished = client.finish();
    assert_eq!(finished.status, Some(1), "{}", finished.stderr);
    assert_eq!(finished.stdout, "");
    assert!(finished.stderr.contains(TERMINATED), "{}", finished.stderr);
}

/// A root whose line 5 is a loop of two statements, followed on line 8 by
/// the statement after it.
const LOOPING: &str = indoc! {"
    class Looping = Object new (
      | count ::= 0. sum ::= 0. |
    ) (
      public main: platform args: arguments = (
        [count < 3] whileTrue: [
          sum:: sum + count.
          count:: count + 1].
        platform transcript show: sum printString; newLine.
      )
    ) : ()
"};

/// A step in from the loop's line stops in its body; a step out of the
/// body leaves the whole loop, which runs its remaining rounds, and stops
/// at the line after it.
#[test]
fn a_step_out_of_a_loop_s_body_stops_after_the_loop() {
    let (_directory, path) = program("Looping", LOOPING);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(true);
    client.stopped("entry");
    assert_eq!(
        top_frame(&mut client),
        (json!("Looping>>main:args:"), json!(5))
    );
    client.succeed("stepIn", json!({ "threadId": 1 }));
    client.stopped("step");
    assert_eq!(
        top_frame(&mut client),
        (json!("Looping>>main:args:"), json!(6))
    );
    client.succeed("stepOut", json!({ "threadId": 1 }));
    client.stopped("step");
    assert_eq!(
        top_frame(&mut client),
        (json!("Looping>>main:args:"), json!(8))
    );
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "3\n");
}

/// A step by line over a send whose callee holds a breakpoint ends at the
/// breakpoint, reported as the breakpoint's stop.
#[test]
fn a_breakpoint_hit_mid_line_step_stops_there() {
    let (_directory, path) = program("Calls", CALLS);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(true);
    let breakpoints = client.set_breakpoints(file_source(&path), &[8]);
    client.configuration_done();
    client.stopped("entry");
    client.succeed("next", json!({ "threadId": 1 }));
    let stopped = client.stopped("breakpoint");
    assert_eq!(
        stopped["body"]["hitBreakpointIds"],
        json!([breakpoints[0]["id"]]),
        "{stopped}"
    );
    assert_eq!(top_frame(&mut client), (json!("Calls>>double:"), json!(8)));
    client.set_breakpoints(file_source(&path), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "42\n");
}

#[test]
fn run_to_cursor_stops_at_the_line() {
    let (_directory, path) = program("Loop", LOOP);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(true);
    client.stopped("entry");
    let refused = client.request(
        "nsNext/runToPosition",
        json!({ "source": { "name": "Elsewhere.ns" }, "line": 1 }),
    );
    assert_eq!(refused["success"], false, "{refused}");
    let past = client.request(
        "nsNext/runToPosition",
        json!({ "source": file_source(&path), "line": 999 }),
    );
    assert_eq!(past["message"], "the module has no such line", "{past}");
    client.succeed(
        "nsNext/runToPosition",
        json!({ "source": file_source(&path), "line": 8 }),
    );
    client.stopped("step");
    let frames = client.stack_frames(0, 0);
    assert_eq!(frames[0]["line"], 8, "{frames:?}");
    let locals = locals_of(&mut client, &frames[0]);
    let variables = client.variables(&locals, None, None);
    assert_eq!(
        variable(&variables, "total")["value"],
        "15",
        "{variables:?}"
    );
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "15\n");
}

/// A method of a shipped module the binary has but builds only on first
/// demand -- `newspeak.core.CollectionsFactory`, which `CALLS` never
/// demands -- is accepted and stays pending, unverified, never refused as
/// a module that never comes.
#[test]
fn a_function_breakpoint_in_a_module_not_built_yet_stays_pending() {
    let (_directory, path) = program("Calls", CALLS);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    let body = client.succeed(
        "setFunctionBreakpoints",
        json!({
            "breakpoints": [{ "name": "newspeak.core.CollectionsFactory/List>>add:" }],
        }),
    );
    let breakpoints = dap::array(&body["breakpoints"]);
    assert_eq!(breakpoints[0]["verified"], false, "{body}");
    assert_eq!(
        breakpoints[0]["message"],
        "pending: the module is not built yet, or its class declares no such method",
        "{body}"
    );
    client.configuration_done();
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "42\n");
}

#[test]
fn a_function_breakpoint_stops_at_the_method_s_entry() {
    let (_directory, path) = program("Calls", CALLS);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    let body = client.succeed(
        "setFunctionBreakpoints",
        json!({
            "breakpoints": [{ "name": "Calls>>double:" }, { "name": "Calls double:" }],
        }),
    );
    let breakpoints = dap::array(&body["breakpoints"]);
    assert_eq!(breakpoints[0]["verified"], true, "{body}");
    assert_eq!(breakpoints[1]["verified"], false, "{body}");
    client.configuration_done();
    let stopped = client.stopped("function breakpoint");
    assert_eq!(
        stopped["body"]["hitBreakpointIds"],
        json!([breakpoints[0]["id"]]),
        "{stopped}"
    );
    assert_eq!(top_frame(&mut client), (json!("Calls>>double:"), json!(8)));
    client.succeed("setFunctionBreakpoints", json!({ "breakpoints": [] }));
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "42\n");
}

/// A root that signals an Error no handler takes.
const FAILS: &str = indoc! {"
    class Fails = Object new () (
      public main: platform args: arguments = (
        platform transcript show: 'before'; newLine.
        Error signal: 'boom'.
      )
    ) : ()
"};

#[test]
fn an_uncaught_exception_stops_with_its_description() {
    let (_directory, path) = program("Fails", FAILS);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    client.succeed(
        "setExceptionBreakpoints",
        json!({
            "filters": [],
            "filterOptions": [
                { "filterId": "uncaught", "condition": "newspeak.core.Kernel/Error, newspeak.core.Kernel/Exception" },
            ],
        }),
    );
    client.configuration_done();
    let stopped = client.stopped("exception");
    assert_eq!(stopped["body"]["text"], "boom", "{stopped}");
    assert_eq!(stopped["body"]["description"], "Error", "{stopped}");
    // The exception's own machinery is shown on demand only.
    let frames = client.stack_frames(0, 0);
    let shown = frames
        .iter()
        .find(|frame| frame.get("presentationHint").is_none())
        .unwrap_or_else(|| panic!("no frame shown: {frames:?}"));
    assert_eq!(shown["name"], "Fails>>main:args:", "{frames:?}");
    assert_eq!(shown["line"], 4, "{frames:?}");
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 1);
    let finished = client.finish();
    assert!(finished.stderr.contains("boom"), "{}", finished.stderr);
}

#[test]
fn pause_stops_a_running_loop() {
    let (_directory, path) = program("Spinner", SPINNER);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(false);
    client.wait_for_stdout("spinning\n");
    client.succeed("pause", json!({ "threadId": 1 }));
    client.stopped("pause");
    assert_eq!(top_frame(&mut client).0, "Spinner>>main:args:");
    terminate_the_spinner(client);
}

#[test]
fn a_stack_trace_while_running_is_an_error() {
    let (_directory, path) = program("Spinner", SPINNER);
    let mut client = DapClient::connect_mode(&path);
    client.start_session(false);
    client.wait_for_stdout("spinning\n");
    for command in ["stackTrace", "next", "stepIn", "stepOut", "continue"] {
        let refused = client.request(command, json!({ "threadId": 1 }));
        assert_eq!(refused["success"], false, "{refused}");
        assert_eq!(refused["message"], "the program is running", "{refused}");
    }
    terminate_the_spinner(client);
}

/// A breakpoint in a shipped module built on first demand is pending until
/// the program builds it, then reported changed under the id it was
/// answered with, and stops.
#[test]
fn a_pending_breakpoint_is_reported_once_its_module_is_built() {
    let (_directory, path) = program("Hello", HELLO);
    let transcript = std::fs::read_to_string(
        common::repository_root()
            .join("crates/ns-runtime/core-library/platform/TranscriptFactory.ns"),
    )
    .expect("read TranscriptFactory.ns");
    let line = transcript
        .lines()
        .position(|line| line.contains("^vmMirror transcript: self show: text ifFail:"))
        .expect("`show:`'s body");
    let line = u32::try_from(line + 1).expect("a line number");
    let (_library_directory, library) = core_library();
    let transcript_file = file_source(&library.join("platform").join("TranscriptFactory.ns"));
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach_with(json!({ "stopOnEntry": false, "coreLibrary": absolute(&library) }));
    let breakpoints = client.set_breakpoints(transcript_file.clone(), &[line]);
    assert_eq!(breakpoints[0]["verified"], false, "{breakpoints:?}");
    client.configuration_done();
    let changed = client.event("breakpoint");
    assert_eq!(changed["body"]["reason"], "changed", "{changed}");
    let breakpoint = &changed["body"]["breakpoint"];
    assert_eq!(breakpoint["id"], breakpoints[0]["id"], "{changed}");
    assert_eq!(breakpoint["verified"], true, "{changed}");
    assert_eq!(breakpoint["line"], line, "{changed}");
    let stopped = client.stopped("breakpoint");
    assert_eq!(
        stopped["body"]["hitBreakpointIds"],
        json!([breakpoints[0]["id"]]),
        "{stopped}"
    );
    client.set_breakpoints(transcript_file, &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
}

/// `evaluate` in the frame `frame_id` -- the top frame when `None` -- in
/// the context `context`; answers the response.
fn evaluate(
    client: &mut DapClient,
    expression: &str,
    frame_id: Option<u64>,
    context: &str,
) -> Value {
    let mut arguments = json!({ "expression": expression, "context": context });
    if let Some(frame_id) = frame_id {
        arguments["frameId"] = json!(frame_id);
    }
    client.request("evaluate", arguments)
}

/// The body of an `evaluate` that must succeed.
fn evaluated(client: &mut DapClient, expression: &str, frame_id: Option<u64>) -> Value {
    let response = evaluate(client, expression, frame_id, "repl");
    assert_eq!(response["success"], true, "{response}");
    response["body"].clone()
}

/// The message of an `evaluate` in the top frame that must fail.
fn evaluation_refused(client: &mut DapClient, expression: &str) -> String {
    let response = evaluate(client, expression, Some(1), "repl");
    assert_eq!(response["success"], false, "{response}");
    response["message"].as_str().unwrap_or_default().to_string()
}

#[test]
fn an_evaluate_answers_the_expression_s_value() {
    let (_directory, path) = program("Stops", STOPS);
    let mut client = DapClient::connect_mode(&path);
    let capabilities = client.initialize()["body"].clone();
    for capability in [
        "supportsConditionalBreakpoints",
        "supportsLogPoints",
        "supportsEvaluateForHovers",
        "supportsSetVariable",
    ] {
        assert_eq!(capabilities[capability], true, "{capabilities}");
    }
    client.attach(false);
    client.set_breakpoints(file_source(&path), &[6]);
    client.configuration_done();
    client.stopped("breakpoint");

    let sum = evaluated(&mut client, "count + 4", Some(1));
    assert_eq!(sum["result"], "7", "{sum}");
    assert!(sum["type"].is_string(), "{sum}");
    assert_eq!(sum["variablesReference"], 0, "{sum}");
    assert!(sum["nsNext/valueReference"].is_i64(), "{sum}");

    // Without a frame, the top frame.
    let elements = evaluated(&mut client, "elements", None);
    assert_eq!(elements["indexedVariables"], 3, "{elements}");
    assert_ne!(elements["variablesReference"], 0, "{elements}");
    assert_ne!(
        elements["variablesReference"], elements["nsNext/valueReference"],
        "{elements}"
    );
    let children = client.variables(&elements["variablesReference"], None, None);
    assert_eq!(children[0]["value"], "10", "{children:?}");

    // A hover and a watch are answered as the console is.
    let hover = evaluate(&mut client, "count", Some(1), "hover");
    assert_eq!(hover["body"]["result"], "3", "{hover}");
    let watch = evaluate(&mut client, "count", Some(1), "watch");
    assert_eq!(watch["body"]["result"], "3", "{watch}");
    let missing = evaluate(&mut client, "count", Some(99), "watch");
    assert_eq!(missing["success"], false, "{missing}");

    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "3\n");
}

#[test]
fn an_evaluate_error_is_a_failed_response() {
    let (_directory, path) = program("Stops", STOPS);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[6]);
    client.stopped("breakpoint");
    let raised = evaluation_refused(&mut client, "count frobnicate");
    assert!(raised.contains("frobnicate"), "{raised}");
    let malformed = evaluation_refused(&mut client, "count +");
    assert!(
        malformed.starts_with("the expression does not compile: "),
        "{malformed}"
    );
    assert_eq!(
        evaluation_refused(&mut client, "^count"),
        "a ^ cannot leave an evaluation"
    );
    // The stop is still there to read.
    assert_eq!(evaluated(&mut client, "count", Some(1))["result"], "3");
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "3\n");
}

/// A hover's evaluation that has not answered within the agent's bound is
/// aborted, the program still stopped and its stop served; a continue
/// then runs the program to its end, stopping nowhere.
#[test]
fn a_hover_evaluation_that_does_not_answer_is_aborted() {
    let (_directory, path) = program("Stops", STOPS);
    let mut client = DapClient::connect_mode_with("run", &path, &[(HOVER_BOUND, "200")]);
    breakpoints_before_the_start(&mut client, &path, &[6]);
    client.stopped("breakpoint");
    let endless = evaluate(&mut client, "[true] whileTrue: []", Some(1), "hover");
    assert_eq!(endless["success"], false, "{endless}");
    assert_eq!(endless["message"], "evaluation aborted", "{endless}");
    assert_eq!(evaluated(&mut client, "count", Some(1))["result"], "3");
    continue_to_the_end(client);
}

/// A `cancel` naming an evaluation the VM has not answered aborts it, the
/// program still stopped and its stop served; a cancel of a request that
/// answered already is moot and answered with success, and a continue then
/// runs the program to its end, stopping nowhere.
#[test]
fn a_cancelled_evaluation_is_aborted() {
    let (_directory, path) = program("Stops", STOPS);
    let mut client = DapClient::connect_mode(&path);
    let capabilities = client.initialize();
    assert_eq!(
        capabilities["body"]["supportsCancelRequest"], true,
        "{capabilities}"
    );
    client.attach(false);
    client.set_breakpoints(file_source(&path), &[6]);
    client.configuration_done();
    client.stopped("breakpoint");
    let endless = client.connection.send(
        "evaluate",
        json!({ "expression": "[true] whileTrue: []", "frameId": 1, "context": "repl" }),
    );
    client.succeed("cancel", json!({ "requestId": endless }));
    let aborted = client.connection.response_to(endless, "evaluate");
    assert_eq!(aborted["success"], false, "{aborted}");
    assert_eq!(aborted["message"], "evaluation aborted", "{aborted}");
    client.succeed("cancel", json!({ "requestId": endless }));
    assert_eq!(evaluated(&mut client, "count", Some(1))["result"], "3");
    continue_to_the_end(client);
}

/// A program that marks its array's stop, then waits in a loop for its
/// slot to be cleared.
const WAITING: &str = indoc! {"
    class Waiting = Object new (
      | waiting ::= true. |
    ) (
      public main: platform args: arguments = (
        | elements |
        elements:: {10. 20. 30}.
        platform transcript show: 'first'; newLine.
        [waiting] whileTrue: [].
        platform transcript show: 'done'; newLine.
      )
    ) : ()
"};

/// A client that leaves a listening agent while an evaluation of its
/// runs leaves the program going on: the evaluation is aborted and the
/// stop left once it answered. The next client meets none of the first's
/// marks.
#[test]
fn a_client_leaving_during_an_evaluation_lets_the_program_go_on() {
    let (_directory, path) = program("Waiting", WAITING);
    let mut client = DapClient::listen_mode(&path);
    client.initialize();
    client.attach(false);
    client.set_breakpoints(file_source(&path), &[7]);
    client.configuration_done();
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 1);
    let locals = locals_of(&mut client, &frames[0]);
    let variables = client.variables(&locals, None, None);
    let reference = variable(&variables, "elements")["nsNext/valueReference"].clone();
    client.succeed(
        "nsNext/mark",
        json!({ "valueReference": reference, "label": "numbers" }),
    );
    client.connection.send(
        "evaluate",
        json!({ "expression": "[true] whileTrue: []", "frameId": 1, "context": "repl" }),
    );
    client.succeed("disconnect", json!({}));
    client.connection.expect_closed();
    client.wait_for_stdout(
        "first
",
    );

    client.reconnect();
    client.start_session(false);
    client.succeed("pause", json!({ "threadId": 1 }));
    client.stopped("pause");
    let marked = evaluate(&mut client, "numbers_DebugLabel", Some(1), "repl");
    assert_eq!(marked["success"], false, "{marked}");
    evaluated(&mut client, "waiting:: false", Some(1));
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(
        client.finish().stdout,
        "first
done
"
    );
}

/// A client that leaves a listening agent while the program runs has its
/// marks taken off at once, before the next client reads the program.
#[test]
fn a_client_leaving_while_the_program_runs_leaves_no_mark() {
    let (_directory, path) = program("Waiting", WAITING);
    let mut client = DapClient::listen_mode(&path);
    client.initialize();
    client.attach(false);
    client.set_breakpoints(file_source(&path), &[7]);
    client.configuration_done();
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 1);
    let locals = locals_of(&mut client, &frames[0]);
    let variables = client.variables(&locals, None, None);
    let reference = variable(&variables, "elements")["nsNext/valueReference"].clone();
    client.succeed(
        "nsNext/mark",
        json!({ "valueReference": reference, "label": "numbers" }),
    );
    client.succeed("continue", json!({ "threadId": 1 }));
    client.wait_for_stdout(
        "first
",
    );
    client.succeed("disconnect", json!({}));
    client.connection.expect_closed();

    client.reconnect();
    client.start_session(false);
    client.succeed("pause", json!({ "threadId": 1 }));
    client.stopped("pause");
    let marked = evaluate(&mut client, "numbers_DebugLabel", Some(1), "repl");
    assert_eq!(marked["success"], false, "{marked}");
    evaluated(&mut client, "waiting:: false", Some(1));
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(
        client.finish().stdout,
        "first
done
"
    );
}

/// A program that marks its array's stop, then sends `tick` in a loop
/// until its slot is cleared.
const TICKING: &str = indoc! {"
    class Ticking = Object new (
      | waiting ::= true. ticks ::= 0. |
    ) (
      tick = (
        ticks:: ticks + 1.
      )
      public main: platform args: arguments = (
        | elements |
        elements:: {10. 20. 30}.
        platform transcript show: 'first'; newLine.
        [waiting] whileTrue: [self tick].
        platform transcript show: 'done'; newLine.
      )
    ) : ()
"};

/// A client that leaves a listening agent while the program runs has its
/// marks taken off at once, before any stop: the next client's breakpoint
/// condition, which the VM decides before it stops, finds no such label,
/// and fails as a condition naming an unknown name does.
#[test]
fn a_departed_client_s_label_is_unknown_to_the_next_client_s_condition() {
    let (_directory, path) = program("Ticking", TICKING);
    let mut client = DapClient::listen_mode(&path);
    client.initialize();
    client.attach(false);
    client.set_breakpoints(file_source(&path), &[10]);
    client.configuration_done();
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 1);
    let locals = locals_of(&mut client, &frames[0]);
    let variables = client.variables(&locals, None, None);
    let reference = variable(&variables, "elements")["nsNext/valueReference"].clone();
    client.succeed(
        "nsNext/mark",
        json!({ "valueReference": reference, "label": "numbers" }),
    );
    client.succeed("continue", json!({ "threadId": 1 }));
    client.wait_for_stdout(
        "first
",
    );
    client.succeed("disconnect", json!({}));
    client.connection.expect_closed();

    client.reconnect();
    client.initialize();
    client.attach(false);
    let body = client.succeed(
        "setBreakpoints",
        json!({
            "source": file_source(&path),
            "breakpoints": [{ "line": 5, "condition": "numbers_DebugLabel size = 3" }],
        }),
    );
    assert_eq!(body["breakpoints"][0]["verified"], true, "{body}");
    client.configuration_done();
    let stopped = client.stopped("breakpoint");
    let text = stopped["body"]["text"].as_str().unwrap_or_default();
    assert!(text.contains("numbers_DebugLabel"), "{stopped}");
    client.set_breakpoints(file_source(&path), &[]);
    evaluated(&mut client, "waiting:: false", Some(1));
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(
        client.finish().stdout,
        "first
done
"
    );
}

/// Evaluations sent together run one at a time: a `cancel` aborts only
/// the one the VM runs, the oldest, while one still queued behind it is
/// answered "cancelled" and never runs, and the others run once the
/// running one ended.
#[test]
fn a_cancel_aborts_only_the_running_evaluation() {
    let (_directory, path) = program("Stops", STOPS);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[6]);
    client.stopped("breakpoint");
    let seqs = client.connection.send_together(&[
        (
            "evaluate",
            json!({ "expression": "[true] whileTrue: []", "frameId": 1, "context": "repl" }),
        ),
        (
            "evaluate",
            json!({ "expression": "count", "frameId": 1, "context": "repl" }),
        ),
        (
            "evaluate",
            json!({ "expression": "count + 1", "frameId": 1, "context": "repl" }),
        ),
    ]);
    client.succeed("cancel", json!({ "requestId": seqs[1] }));
    let cancelled = client.connection.response_to(seqs[1], "evaluate");
    assert_eq!(cancelled["success"], false, "{cancelled}");
    assert_eq!(cancelled["message"], "cancelled", "{cancelled}");
    client.succeed("cancel", json!({ "requestId": seqs[0] }));
    let aborted = client.connection.response_to(seqs[0], "evaluate");
    assert_eq!(aborted["message"], "evaluation aborted", "{aborted}");
    let third = client.connection.response_to(seqs[2], "evaluate");
    assert_eq!(third["body"]["result"], "4", "{third}");
    continue_to_the_end(client);
}

/// The variable through which a debug build's agent takes its bound of a
/// hover's evaluation, in milliseconds.
const HOVER_BOUND: &str = "NS_NEXT_DEBUG_HOVER_BOUND_MILLISECONDS";

/// Hovers whose bound passes as they are asked -- a bound of 0 -- race
/// their abort with their own end: each answers its value or is aborted,
/// and an abort that arrives after the answer is moot, leaving nothing
/// that stops the program again once it goes on.
#[test]
fn a_hover_answered_around_its_bound_stops_nothing_later() {
    let (_directory, path) = program("Stops", STOPS);
    let mut client = DapClient::connect_mode_with("run", &path, &[(HOVER_BOUND, "0")]);
    breakpoints_before_the_start(&mut client, &path, &[6]);
    client.stopped("breakpoint");
    for _ in 0..20 {
        let hover = evaluate(&mut client, "count", Some(1), "hover");
        if hover["success"] == true {
            assert_eq!(hover["body"]["result"], "3", "{hover}");
        } else {
            assert_eq!(hover["message"], "evaluation aborted", "{hover}");
        }
    }
    assert_eq!(evaluated(&mut client, "count", Some(1))["result"], "3");
    continue_to_the_end(client);
}

/// Continues a stopped program, which must end with status 0, printing
/// `3`, and stop nowhere on the way.
fn continue_to_the_end(mut client: DapClient) {
    client.succeed("continue", json!({ "threadId": 1 }));
    let events = client.connection.events_until_closed();
    let names = names(&events);
    assert!(
        !names.iter().any(|name| name == "stopped"),
        "the program stopped again: {names:?}"
    );
    let exited = names
        .iter()
        .position(|name| name == "exited")
        .unwrap_or_else(|| panic!("no `exited` event: {names:?}"));
    assert_eq!(events[exited]["body"]["exitCode"], 0, "{names:?}");
    assert_eq!(client.finish().stdout, "3\n");
}

#[test]
fn a_set_variable_changes_what_the_program_prints() {
    let (_directory, path) = program("Stops", STOPS);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[6]);
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 1);
    let locals = locals_of(&mut client, &frames[0]);
    let set = client.succeed(
        "setVariable",
        json!({ "variablesReference": locals, "name": "count", "value": "40 + 2" }),
    );
    assert_eq!(set["value"], "42", "{set}");
    assert_eq!(set["variablesReference"], 0, "{set}");
    let variables = client.variables(&locals, None, None);
    assert_eq!(
        variable(&variables, "count")["value"],
        "42",
        "{variables:?}"
    );

    let elements = variable(&variables, "elements")["variablesReference"].clone();
    let refused = client.request(
        "setVariable",
        json!({ "variablesReference": elements, "name": "[1]", "value": "0" }),
    );
    assert_eq!(refused["success"], false, "{refused}");
    assert_eq!(
        refused["message"], "only a frame's variables are set",
        "{refused}"
    );
    let failing = client.request(
        "setVariable",
        json!({ "variablesReference": locals, "name": "count", "value": "count frobnicate" }),
    );
    assert_eq!(failing["success"], false, "{failing}");
    let message = failing["message"].as_str().unwrap_or_default();
    assert!(message.contains("frobnicate"), "{failing}");

    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "42\n");
}

#[test]
fn a_conditional_breakpoint_stops_only_where_it_holds() {
    let (_directory, path) = program("Loop", LOOP);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    let body = client.succeed(
        "setBreakpoints",
        json!({
            "source": file_source(&path),
            "breakpoints": [{ "line": 6, "condition": "index = 4" }],
        }),
    );
    let breakpoint = body["breakpoints"][0].clone();
    assert_eq!(breakpoint["verified"], true, "{body}");
    // A method breakpoint whose condition never holds stops neither at
    // the method's entry nor at its exit.
    let functions = client.succeed(
        "setFunctionBreakpoints",
        json!({ "breakpoints": [{ "name": "Loop>>main:args:", "condition": "false" }] }),
    );
    assert_eq!(functions["breakpoints"][0]["verified"], true, "{functions}");
    client.configuration_done();
    let stopped = client.stopped("breakpoint");
    assert_eq!(
        stopped["body"]["hitBreakpointIds"],
        json!([breakpoint["id"]]),
        "{stopped}"
    );
    assert_eq!(evaluated(&mut client, "index", Some(1))["result"], "4");
    client.succeed("continue", json!({ "threadId": 1 }));
    let events = client.connection.events_until_closed();
    assert!(
        !names(&events).contains(&"stopped".to_string()),
        "{events:?}"
    );
    assert_eq!(client.finish().stdout, "15\n");
}

#[test]
fn a_failing_condition_stops_with_its_error() {
    let (_directory, path) = program("Loop", LOOP);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    let body = client.succeed(
        "setBreakpoints",
        json!({
            "source": file_source(&path),
            "breakpoints": [{ "line": 6, "condition": "index frobnicate" }],
        }),
    );
    let breakpoint = body["breakpoints"][0].clone();
    client.configuration_done();
    let stopped = client.stopped("breakpoint");
    assert_eq!(
        stopped["body"]["hitBreakpointIds"],
        json!([breakpoint["id"]]),
        "{stopped}"
    );
    let text = stopped["body"]["text"].as_str().unwrap_or_default();
    assert!(text.contains("frobnicate"), "{stopped}");
    client.set_breakpoints(file_source(&path), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "15\n");
}

#[test]
fn a_logpoint_writes_an_output_event_and_does_not_stop() {
    let (_directory, path) = program("Loop", LOOP);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    let body = client.succeed(
        "setBreakpoints",
        json!({
            "source": file_source(&path),
            "breakpoints": [
                { "line": 6, "logMessage": "at {index} {{of}} { {1. 2} size + 3}" },
                { "line": 8, "logMessage": "total {total" },
            ],
        }),
    );
    let breakpoints = dap::array(&body["breakpoints"]);
    assert_eq!(breakpoints[0]["verified"], true, "{body}");
    assert_eq!(breakpoints[1]["verified"], false, "{body}");
    let message = breakpoints[1]["message"].as_str().unwrap_or_default();
    assert!(message.contains("unclosed `{`"), "{body}");
    client.configuration_done();
    for index in 1..=5 {
        let output = client.event("output");
        assert_eq!(output["body"]["category"], "console", "{output}");
        assert_eq!(
            output["body"]["output"],
            format!("at {index} {{of}} 5\n"),
            "{output}"
        );
    }
    let events = client.connection.events_until_closed();
    assert!(
        !names(&events).contains(&"stopped".to_string()),
        "{events:?}"
    );
    assert_eq!(client.finish().stdout, "15\n");
}

/// A root that holds an Array in a local at line 5, and nil at line 6.
const MARKS: &str = indoc! {"
    class Marks = Object new () (
      public main: platform args: arguments = (
        | elements |
        elements:: {10. 20. 30}.
        elements:: nil.
        platform transcript show: 'done'; newLine.
      )
    ) : ()
"};

#[test]
fn a_marked_object_is_named_in_a_later_evaluate() {
    let (_directory, path) = program("Marks", MARKS);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[5, 6]);
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 1);
    let locals = locals_of(&mut client, &frames[0]);
    let variables = client.variables(&locals, None, None);
    let elements = variable(&variables, "elements").clone();
    let reference = elements["nsNext/valueReference"].clone();
    assert!(reference.is_i64(), "{elements}");
    client.succeed(
        "nsNext/mark",
        json!({ "valueReference": reference, "label": "numbers" }),
    );
    let variables = client.variables(&locals, None, None);
    let marked = variable(&variables, "elements")["value"].clone();
    assert!(
        marked
            .as_str()
            .is_some_and(|value| value.ends_with(" [numbers]")),
        "{marked}"
    );
    let refused = client.request(
        "nsNext/mark",
        json!({ "valueReference": reference, "label": "not a label" }),
    );
    assert_eq!(refused["success"], false, "{refused}");

    client.succeed("continue", json!({ "threadId": 1 }));
    client.stopped("breakpoint");
    // A reference of the earlier stop names nothing.
    let stale = client.request(
        "nsNext/mark",
        json!({ "valueReference": reference, "label": "again" }),
    );
    assert_eq!(stale["success"], false, "{stale}");
    let size = evaluated(&mut client, "numbers_DebugLabel size", Some(1));
    assert_eq!(size["result"], "3", "{size}");
    let named = evaluated(&mut client, "numbers_DebugLabel", Some(1));
    assert!(
        named["result"]
            .as_str()
            .is_some_and(|value| value.ends_with(" [numbers]")),
        "{named}"
    );
    client.succeed("nsNext/unmark", json!({ "label": "numbers" }));
    let gone = evaluate(&mut client, "numbers_DebugLabel size", Some(1), "repl");
    assert_eq!(gone["success"], false, "{gone}");

    client.set_breakpoints(file_source(&path), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "done\n");
}

/// Every value of a shipped frame is withheld, so read-only and without a
/// value reference; a program frame's are not.
#[test]
fn a_withheld_value_is_read_only() {
    let (_directory, path) = program("Walker", WALKER);
    let (_library_directory, library) = core_library();
    let kernel = file_source(&library.join("Kernel.ns"));
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach_with(json!({ "stopOnEntry": false, "coreLibrary": absolute(&library) }));
    client.set_breakpoints(kernel.clone(), &[array_do_body_line()]);
    client.configuration_done();
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 0);
    assert_eq!(
        frames[0]["name"], "newspeak.core.Kernel/Array>>do:",
        "{frames:?}"
    );
    let shipped = locals_of(&mut client, &frames[0]);
    let variables = client.variables(&shipped, None, None);
    let receiver = variable(&variables, "self");
    assert_eq!(
        receiver["presentationHint"]["attributes"],
        json!(["readOnly"]),
        "{receiver}"
    );
    assert!(
        receiver.get("nsNext/valueReference").is_none(),
        "{receiver}"
    );

    let program = frames
        .iter()
        .find(|frame| frame["name"] == "Walker>>main:args:")
        .unwrap_or_else(|| panic!("no program frame: {frames:?}"));
    let locals = locals_of(&mut client, program);
    let variables = client.variables(&locals, None, None);
    let receiver = variable(&variables, "self");
    assert!(receiver.get("presentationHint").is_none(), "{receiver}");
    assert!(receiver["nsNext/valueReference"].is_i64(), "{receiver}");

    client.set_breakpoints(kernel, &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "12\n");
}

/// A root whose line 4 sends `second`, then `first:` with its answer.
const SMART: &str = indoc! {"
    class Smart = Object new () (
      public main: platform args: arguments = (
        | value |
        value:: self first: (self second).
        platform transcript show: value printString; newLine.
      )
      first: number = (
        ^number + 1
      )
      second = (
        ^41
      )
    ) : ()
"};

/// The 1-based UTF-16 column at which `token` begins on the 1-based line
/// `line` of `text`.
fn column_of(text: &str, line: usize, token: &str) -> usize {
    let line = text.lines().nth(line - 1).expect("the line");
    let before = &line[..line.find(token).expect("the token")];
    before.encode_utf16().count() + 1
}

#[test]
fn step_in_targets_list_the_line_s_sends() {
    let (_directory, path) = program("Smart", SMART);
    let mut client = DapClient::connect_mode(&path);
    let capabilities = client.initialize()["body"].clone();
    assert_eq!(
        capabilities["supportsStepInTargetsRequest"], true,
        "{capabilities}"
    );
    client.attach(false);
    client.set_breakpoints(file_source(&path), &[4]);
    client.configuration_done();
    client.stopped("breakpoint");
    let body = client.succeed("stepInTargets", json!({ "frameId": 1 }));
    let targets = dap::array(&body["targets"]);
    let labels: Vec<&Value> = targets.iter().map(|target| &target["label"]).collect();
    assert_eq!(labels, [&json!("second"), &json!("first:")], "{body}");
    let second = column_of(SMART, 4, "second");
    assert_eq!(targets[0]["line"], 4, "{body}");
    assert_eq!(targets[0]["column"], second, "{body}");
    assert_eq!(targets[0]["endLine"], 4, "{body}");
    assert_eq!(targets[0]["endColumn"], second + "second".len(), "{body}");
    let first = column_of(SMART, 4, "first:");
    assert_eq!(targets[1]["column"], first, "{body}");
    assert_eq!(targets[1]["endColumn"], first + "first:".len(), "{body}");
    assert_ne!(targets[0]["id"], targets[1]["id"], "{body}");
    let missing = client.request("stepInTargets", json!({ "frameId": 7 }));
    assert_eq!(missing["success"], false, "{missing}");

    client.set_breakpoints(file_source(&path), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "42\n");
}

#[test]
fn step_in_with_a_target_stops_in_it() {
    let (_directory, path) = program("Smart", SMART);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[4]);
    client.stopped("breakpoint");
    let body = client.succeed("stepInTargets", json!({ "frameId": 1 }));
    let targets = dap::array(&body["targets"]);
    let first = targets
        .iter()
        .find(|target| target["label"] == "first:")
        .unwrap_or_else(|| panic!("no target `first:`: {body}"));
    client.succeed("stepIn", json!({ "threadId": 1, "targetId": first["id"] }));
    client.stopped("step");
    let frames = client.stack_frames(0, 0);
    assert_eq!(frames.len(), 2, "{frames:?}");
    assert_eq!(frames[0]["name"], "Smart>>first:", "{frames:?}");
    assert_eq!(frames[0]["line"], 8, "{frames:?}");
    // Only the frame a step steps from has targets.
    let below = client.request("stepInTargets", json!({ "frameId": 2 }));
    assert_eq!(below["success"], false, "{below}");
    assert_eq!(
        below["message"], "smart step into takes the top frame",
        "{below}"
    );
    let unknown = client.request("stepIn", json!({ "threadId": 1, "targetId": 99 }));
    assert_eq!(unknown["success"], false, "{unknown}");

    client.set_breakpoints(file_source(&path), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "42\n");
}

/// A root whose `twice:` prints `in` on line 6 before it answers 42 on
/// line 7.
const AGAIN: &str = indoc! {"
    class Again = Object new () (
      public main: platform args: arguments = (
        platform transcript show: (self twice: platform) printString; newLine.
      )
      twice: platform = (
        platform transcript show: 'in'; newLine.
        ^21 * 2
      )
    ) : ()
"};

#[test]
fn a_restart_frame_runs_the_method_again() {
    let (_directory, path) = program("Again", AGAIN);
    let mut client = DapClient::connect_mode(&path);
    let capabilities = client.initialize()["body"].clone();
    assert_eq!(capabilities["supportsRestartFrame"], true, "{capabilities}");
    client.attach(false);
    client.set_breakpoints(file_source(&path), &[7]);
    client.configuration_done();
    client.stopped("breakpoint");
    assert_eq!(top_frame(&mut client), (json!("Again>>twice:"), json!(7)));
    let missing = client.request("restartFrame", json!({ "frameId": 9 }));
    assert_eq!(missing["success"], false, "{missing}");
    client.succeed("restartFrame", json!({ "frameId": 1 }));
    client.stopped("restart");
    assert_eq!(top_frame(&mut client), (json!("Again>>twice:"), json!(6)));
    client.set_breakpoints(file_source(&path), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "in\nin\n42\n");
}

#[test]
fn a_forced_return_answers_the_expression_s_value() {
    let (_directory, path) = program("Again", AGAIN);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[7]);
    client.stopped("breakpoint");
    // An expression that fails forces nothing: the program stays stopped.
    let failing = client.request(
        "nsNext/forceReturn",
        json!({ "frameId": 1, "expression": "frobnicate" }),
    );
    assert_eq!(failing["success"], false, "{failing}");
    let message = failing["message"].as_str().unwrap_or_default();
    assert!(message.contains("frobnicate"), "{failing}");
    assert_eq!(top_frame(&mut client), (json!("Again>>twice:"), json!(7)));

    client.succeed(
        "nsNext/forceReturn",
        json!({ "frameId": 1, "expression": "3 + 4" }),
    );
    client.stopped("step");
    assert_eq!(top_frame(&mut client).0, json!("Again>>main:args:"));
    client.set_breakpoints(file_source(&path), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "in\n7\n");
}

/// A forced return sent while an evaluation runs waits for it, as a
/// second evaluation does: once a `cancel` aborted the running one, the
/// forced return evaluates its own expression and forces its value.
#[test]
fn a_forced_return_sent_during_an_evaluation_forces_its_own_value() {
    let (_directory, path) = program("Again", AGAIN);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[7]);
    client.stopped("breakpoint");
    let seqs = client.connection.send_together(&[
        (
            "evaluate",
            json!({ "expression": "[true] whileTrue: [3]", "frameId": 1, "context": "repl" }),
        ),
        (
            "nsNext/forceReturn",
            json!({ "frameId": 1, "expression": "3 + 4" }),
        ),
    ]);
    client.succeed("cancel", json!({ "requestId": seqs[0] }));
    let aborted = client.connection.response_to(seqs[0], "evaluate");
    assert_eq!(aborted["message"], "evaluation aborted", "{aborted}");
    let forced = client.connection.response_to(seqs[1], "nsNext/forceReturn");
    assert_eq!(forced["success"], true, "{forced}");
    client.stopped("step");
    client.set_breakpoints(file_source(&path), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "in\n7\n");
}

/// A root that holds three instances of its nested `Point` in an Array at
/// line 6.
const CENSUS: &str = indoc! {"
    class Census = Object new () (
      class Point = Object new () () : ()
      public main: platform args: arguments = (
        | points |
        points:: {Point new. Point new. Point new}.
        platform transcript show: points size printString; newLine.
      )
    ) : ()
"};

/// A root that holds 1,500 instances of its nested `Point` at line 6.
const CROWD: &str = indoc! {"
    class Crowd = Object new () (
      class Point = Object new () () : ()
      public main: platform args: arguments = (
        | points |
        points:: Array new: 1500. 1 to: 1500 do: [:index | points at: index put: Point new].
        platform transcript show: points size printString; newLine.
      )
    ) : ()
"};

/// `nsNext/instances` and `nsNext/referrers` without a `count` answer at
/// most 1,000, never a class's every instance in one response.
#[test]
fn a_page_without_a_count_is_at_most_a_thousand() {
    let (_directory, path) = program("Crowd", CROWD);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[6]);
    client.stopped("breakpoint");
    let instances = client.succeed("nsNext/instances", json!({ "name": "Crowd/Point" }));
    let instances = dap::array(&instances["variables"]);
    assert_eq!(instances.len(), 1_000);
    let rest = client.succeed(
        "nsNext/instances",
        json!({ "name": "Crowd/Point", "start": 1_000 }),
    );
    assert_eq!(dap::array(&rest["variables"]).len(), 500);
    client.set_breakpoints(file_source(&path), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "1500\n");
}

#[test]
fn instance_counts_name_the_program_s_classes() {
    let (_directory, path) = program("Census", CENSUS);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[6]);
    client.stopped("breakpoint");
    let counts = client.succeed("nsNext/instanceCounts", json!({}));
    let classes = dap::array(&counts["classes"]);
    let point = classes
        .iter()
        .find(|class| class["name"] == "Census/Point")
        .unwrap_or_else(|| panic!("no `Census/Point`: {counts}"));
    assert_eq!(point["count"], 3, "{point}");
    assert!(point.get("nsNext/shipped").is_none(), "{point}");
    assert!(
        classes
            .iter()
            .any(|class| class["name"] == "newspeak.core.Kernel/Array"
                && class["nsNext/shipped"] == true),
        "{counts}"
    );

    let instances = client.succeed(
        "nsNext/instances",
        json!({ "name": "Census/Point", "start": 1, "count": 5 }),
    );
    let instances = dap::array(&instances["variables"]);
    let names: Vec<&Value> = instances.iter().map(|instance| &instance["name"]).collect();
    assert_eq!(names, [&json!("#2"), &json!("#3")], "{instances:?}");
    let reference = instances[0]["nsNext/valueReference"].clone();
    assert!(reference.is_i64(), "{instances:?}");

    // A Point is held by the Array, which the frame's `points` holds.
    let referrers = client.succeed(
        "nsNext/referrers",
        json!({ "valueReference": reference, "start": 0, "count": 10 }),
    );
    let referrers = dap::array(&referrers["variables"]);
    assert!(
        referrers.iter().any(|referrer| referrer["type"] == "Array"),
        "{referrers:?}"
    );
    let frames = client.stack_frames(0, 1);
    let locals = locals_of(&mut client, &frames[0]);
    let variables = client.variables(&locals, None, None);
    let points = variable(&variables, "points")["nsNext/valueReference"].clone();
    let referrers = client.succeed(
        "nsNext/referrers",
        json!({ "valueReference": points, "start": 0, "count": 10 }),
    );
    let referrers = dap::array(&referrers["variables"]);
    let on_stack = variable(&referrers, "frame 1 points");
    assert_eq!(on_stack["variablesReference"], 0, "{on_stack}");
    assert_eq!(
        on_stack["presentationHint"]["kind"], "virtual",
        "{on_stack}"
    );
    assert!(
        on_stack.get("nsNext/valueReference").is_none(),
        "{on_stack}"
    );

    client.set_breakpoints(file_source(&path), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "3\n");
}

#[test]
fn a_shipped_frame_is_deemphasized() {
    let (_directory, path) = program("Walker", WALKER);
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(true);
    client.succeed(
        "nsNext/setStepFilters",
        json!({ "shipped": false, "entries": [] }),
    );
    client.configuration_done();
    client.stopped("entry");
    client.succeed("stepIn", json!({ "threadId": 1 }));
    client.stopped("step");
    let frames = client.stack_frames(0, 0);
    let shipped = &frames[0];
    assert_eq!(
        shipped["name"], "newspeak.core.Kernel/Array>>do:",
        "{frames:?}"
    );
    assert_eq!(shipped["presentationHint"], "subtle", "{shipped}");
    assert_eq!(
        shipped["source"]["presentationHint"], "deemphasize",
        "{shipped}"
    );
    assert_eq!(shipped["nsNext/shipped"], true, "{shipped}");
    assert_eq!(shipped["canRestart"], false, "{shipped}");
    // The VM neither restarts nor forces a frame of the core library.
    for (command, arguments) in [
        ("restartFrame", json!({ "frameId": 1 })),
        ("nsNext/forceReturn", json!({ "frameId": 1 })),
    ] {
        let refused = client.request(command, arguments);
        assert_eq!(refused["success"], false, "{refused}");
        assert_eq!(
            refused["message"], "the frame is the core library's",
            "{refused}"
        );
    }
    let program = frames
        .iter()
        .find(|frame| frame["name"] == "Walker>>main:args:")
        .unwrap_or_else(|| panic!("no program frame: {frames:?}"));
    assert!(program.get("presentationHint").is_none(), "{program}");
    assert!(
        program["source"].get("presentationHint").is_none(),
        "{program}"
    );
    assert!(program.get("nsNext/shipped").is_none(), "{program}");

    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "12\n");
}

#[test]
fn a_step_in_target_of_an_earlier_stop_is_refused() {
    let (_directory, path) = program("Smart", SMART);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[4, 5]);
    client.stopped("breakpoint");
    let body = client.succeed("stepInTargets", json!({ "frameId": 1 }));
    let first = dap::array(&body["targets"])[0]["id"].clone();
    client.succeed("continue", json!({ "threadId": 1 }));
    client.stopped("breakpoint");
    // The VM's ids are positions, which this stop would give another
    // target: the agent sends nothing, and the program stays stopped.
    let stale = client.request("stepIn", json!({ "threadId": 1, "targetId": first }));
    assert_eq!(stale["success"], false, "{stale}");
    assert_eq!(stale["message"], "no such target", "{stale}");
    assert_eq!(
        top_frame(&mut client),
        (json!("Smart>>main:args:"), json!(5))
    );

    client.set_breakpoints(file_source(&path), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "42\n");
}

#[test]
fn a_forced_return_without_an_expression_answers_nil() {
    for arguments in [
        json!({ "frameId": 1 }),
        json!({ "frameId": 1, "expression": "  " }),
    ] {
        let (_directory, path) = program("Again", AGAIN);
        let mut client = DapClient::connect_mode(&path);
        breakpoints_before_the_start(&mut client, &path, &[7]);
        client.stopped("breakpoint");
        client.succeed("nsNext/forceReturn", arguments);
        client.stopped("step");
        assert_eq!(top_frame(&mut client).0, json!("Again>>main:args:"));
        client.set_breakpoints(file_source(&path), &[]);
        client.succeed("continue", json!({ "threadId": 1 }));
        assert_eq!(exit_code_reported(&mut client), 0);
        assert_eq!(client.finish().stdout, "in\nnil\n");
    }
}

/// While a forced return's expression runs the stop counts as left: a
/// `continue` sent meanwhile is refused, and the run still ends on a
/// `terminate`. The VM refuses a command that arrives while an evaluation
/// runs in the same words, so this holds the session's end to account;
/// the agent's own refusal, which keeps a `continue` from crossing the
/// forced return when the evaluation answers first, is pinned by
/// `agent.rs`'s `a_forced_return_passes_the_vm_s_refusal_of_a_withheld_value_on`.
#[test]
fn a_forced_return_s_evaluation_leaves_the_stop() {
    let (_directory, path) = program("Again", AGAIN);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[7]);
    client.stopped("breakpoint");
    let forcing = client.connection.send(
        "nsNext/forceReturn",
        json!({ "frameId": 1, "expression": "[true] whileTrue: []" }),
    );
    let refused = client.request("continue", json!({ "threadId": 1 }));
    assert_eq!(refused["success"], false, "{refused}");
    assert_eq!(refused["message"], "the program is running", "{refused}");
    client.succeed("terminate", json!({}));
    let forced = client.connection.response_to(forcing, "nsNext/forceReturn");
    assert_eq!(forced["success"], false, "{forced}");
    assert_eq!(exit_code_reported(&mut client), 1);
    assert_eq!(client.finish().status, Some(1));
}

#[test]
fn instances_of_a_shipped_class_are_asked_for_by_its_module() {
    let (_directory, path) = program("Kernel", OWN_KERNEL);
    let mut client = DapClient::connect_mode(&path);
    breakpoints_before_the_start(&mut client, &path, &[5]);
    client.stopped("breakpoint");
    // The program's `Kernel` declares no `Array`; the shipped one does.
    let own = client.succeed(
        "nsNext/instances",
        json!({ "name": "Kernel/Array", "start": 0, "count": 0 }),
    );
    assert_eq!(own["variables"], json!([]), "{own}");
    let shipped = client.succeed(
        "nsNext/instances",
        json!({ "name": "newspeak.core.Kernel/Array", "start": 0, "count": 0 }),
    );
    let shipped = dap::array(&shipped["variables"]);
    assert!(!shipped.is_empty(), "{shipped:?}");
    assert!(
        shipped.iter().all(|instance| instance["type"] == "Array"),
        "{shipped:?}"
    );
    // The program's root names its own module.
    let roots = client.succeed(
        "nsNext/instances",
        json!({ "name": "Kernel", "start": 0, "count": 0 }),
    );
    let roots = dap::array(&roots["variables"]);
    assert_eq!(roots.len(), 1, "{roots:?}");
    assert_eq!(roots[0]["type"], "Kernel", "{roots:?}");

    client.set_breakpoints(file_source(&path), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "6\n");
}
