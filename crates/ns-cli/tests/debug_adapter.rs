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
      | Shared = manifest Shared. |
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
        json!({ "shipped": false, "classes": [] }),
    );
    client.configuration_done();
    client.stopped("entry");
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
    // Every frame: the program's module's text is asked of the VM, a
    // shipped module's never is.
    client.stack_frames(0, 0);

    client.succeed("stepIn", json!({ "threadId": 1 }));
    client.stopped("step");
    let frames = client.stack_frames(0, 0);
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

#[test]
fn a_breakpoint_in_a_shipped_module_set_before_the_start_stops() {
    let (_directory, path) = program("Walker", WALKER);
    let kernel = std::fs::read_to_string(
        common::repository_root().join("crates/ns-runtime/core-library/Kernel.ns"),
    )
    .expect("read Kernel.ns");
    // `Array>>do:`'s loop body, which `WALKER`'s `do:` reaches.
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
    let line = u32::try_from(body + 1).expect("a line number");
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    let breakpoints = client.set_breakpoints(json!({ "name": "Kernel.ns" }), &[line]);
    assert_eq!(breakpoints[0]["verified"], true, "{breakpoints:?}");
    assert_eq!(breakpoints[0]["line"], line, "{breakpoints:?}");
    client.configuration_done();
    client.stopped("breakpoint");
    let frames = client.stack_frames(0, 1);
    assert_eq!(frames[0]["name"], "Array>>do:", "{frames:?}");
    assert_eq!(frames[0]["line"], line, "{frames:?}");
    assert_eq!(frames[0]["source"]["name"], "Kernel.ns", "{frames:?}");
    client.set_breakpoints(json!({ "name": "Kernel.ns" }), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
    assert_eq!(client.finish().stdout, "12\n");
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
    let value = client.variables(&returned["variablesReference"], None, None);
    assert_eq!(value[0]["value"], "42", "{value:?}");

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
                .filter(|frame| frame["name"] == "Part class>>new")
                .map(|frame| frame["line"].clone()),
        );
        stops.push(names);
    }
    assert_eq!(
        stops,
        [
            vec!["Parts>>main:args:"],
            vec!["Part class>>new", "Parts>>main:args:"],
            vec!["Part initializer", "Part class>>new", "Parts>>main:args:"],
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
                .filter(|frame| frame["name"] == "Base initializer")
                .map(|frame| frame["line"].clone()),
        );
        stops.push(names);
    }
    assert_eq!(
        stops,
        [
            vec![
                "Base superclass clause",
                "Part superclass clause",
                "DerivedParts>>main:args:"
            ],
            vec!["Part superclass clause", "DerivedParts>>main:args:"],
            vec!["DerivedParts>>main:args:"],
            vec!["Part class>>new", "DerivedParts>>main:args:"],
            vec![
                "Part initializer",
                "Part class>>new",
                "DerivedParts>>main:args:"
            ],
            vec![
                "Base initializer",
                "Part initializer",
                "Part class>>new",
                "DerivedParts>>main:args:"
            ],
            vec![
                "Part initializer",
                "Part class>>new",
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
    // The entry stands in `Punkt`'s superclass clause, which `Punkt x: 1 y: 2`
    // evaluates first; two steps into reach `main:args:`'s statement.
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
            ("Punkt class>>x:y:".to_owned(), 5, 15, 5, 26),
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
        ("Punkt initializer".into(), 5, 29, 5, 42)
    );
    assert_eq!(frames.len(), 3, "{frames:?}");

    let frames = step_to_top(&mut client, "stepIn");
    assert_eq!(
        placed(&frames[0]),
        ("Basis initializer".into(), 2, 15, 2, 21)
    );
    assert_eq!(frames.len(), 4, "{frames:?}");
    let locals = locals_of(&mut client, &frames[0]);
    let variables = client.variables(&locals, None, None);
    assert_eq!(variable(&variables, "a")["value"], "1", "{variables:?}");

    let frames = step_to_top(&mut client, "stepOut");
    assert_eq!(
        placed(&frames[0]),
        ("Punkt initializer".into(), 6, 7, 6, 23)
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
                { "filterId": "uncaught", "condition": "Kernel.Error, Kernel.Exception" },
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
    let mut client = DapClient::connect_mode(&path);
    client.initialize();
    client.attach(false);
    let breakpoints = client.set_breakpoints(json!({ "name": "TranscriptFactory.ns" }), &[line]);
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
    client.set_breakpoints(json!({ "name": "TranscriptFactory.ns" }), &[]);
    client.succeed("continue", json!({ "threadId": 1 }));
    assert_eq!(exit_code_reported(&mut client), 0);
}
