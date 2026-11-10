//! The debugging API at the run loop's safepoint: a host attaches a
//! debugger, pauses a running program from another thread, and continues
//! or terminates it, and reads its stack and values while it is stopped; a
//! command queued while the program runs is served at the next safepoint
//! (`docs/design/debugger.md`, decisions 7 and 9, "Commands while the
//! program runs", "Reading the stack at a stop", "Values the IDE holds").

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use indoc::indoc;
use ns_runtime::debugger::FrameKind;
use ns_runtime::JitStatistics;
use ns_runtime::{
    Command, DebugRemote, Event, FrameDescription, HostResult, HostSignal, Interpreter, Root,
    StopReason, Symbol, ValueKind, Variable,
};

/// How long the remote's thread waits for an event before it fails the
/// test: long enough for a debug build under a loaded machine.
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

/// The message of the error a run the debugger terminated ends with.
const TERMINATED: &str = "the debugger terminated the program";

/// A loop that ends: a run continued after a stop answers its count. Its
/// tests pause before the run or never, so the count need not outlast a
/// thread's sleep, and it stays small enough for the stress modes, under
/// which every backward jump takes the safepoint.
const COUNTING: &str = indoc! {"
    class Counting = Object new (
      | count ::= 0. |
    ) (
      public run = (
        [count < 1000000] whileTrue: [count:: count + 1].
        ^count
      )
    ) : ()
"};

/// A loop that never ends: only the debugger's termination ends its run,
/// so a pause cannot come too late to find it running.
const FOREVER: &str = indoc! {"
    class Forever = Object new (
      | count ::= 0. |
    ) (
      public run = (
        [true] whileTrue: [count:: count + 1].
        ^count
      )
    ) : ()
"};

/// A loop that never ends inside an `ensure:`, whose block a termination
/// runs on its way out.
const ENSURING: &str = indoc! {"
    class Ensuring = Object new (
      | count ::= 0. ensureRan ::= false. |
    ) (
      public run = (
        [[true] whileTrue: [count:: count + 1]] ensure: [ensureRan:: true].
        ^count
      )
      public ensured = ( ^ensureRan )
    ) : ()
"};

/// A loop that allocates without end, so that collections are requested
/// while it runs; only the debugger's termination ends its run, so a pause
/// cannot come too late to find it running, and no stress mode, under which
/// every backward jump takes the safepoint, waits for it to finish.
const ALLOCATING: &str = indoc! {"
    class Allocating = Object new (
      | count ::= 0. |
    ) (
      public run = (
        | items |
        [true] whileTrue: [
          items:: Array new: 100.
          count:: count + 1
        ].
        ^count
      )
    ) : ()
"};

fn load(source: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
}

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// An instance of `source`'s module class, ready for its `run`.
fn instance_of(source: &str) -> (Interpreter, Root) {
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    (interpreter, instance)
}

fn small_integer(interpreter: &mut Interpreter, root: &Root) -> i64 {
    interpreter.enter(|mutator| match mutator.fetch(root).kind() {
        ValueKind::SmallInteger(value) => value,
        _ => panic!("a small integer was expected"),
    })
}

fn run(interpreter: &mut Interpreter, instance: &Root) -> HostResult {
    interpreter.send(instance, Symbol::intern("run"), &[])
}

/// Asserts that `result` is the error a termination ends a run with.
fn assert_terminated(result: HostResult) {
    match result {
        Err(HostSignal::Error(error)) => assert_eq!(error.message.as_str(), TERMINATED),
        Err(HostSignal::Return) => panic!("a non-local return escaped"),
        Ok(_) => panic!("the run finished instead of being terminated"),
    }
}

/// Waits for the stop a pause makes.
fn expect_paused(remote: &DebugRemote) {
    match remote.next_event(EVENT_TIMEOUT) {
        Some(Event::Stopped {
            reason: StopReason::Pause,
        }) => {}
        other => panic!("a stop for the pause was expected, not {other:?}"),
    }
}

/// Continues a stopped program and waits for the confirmation.
fn continue_stopped(remote: &DebugRemote) {
    remote.send(Command::Continue);
    match remote.next_event(EVENT_TIMEOUT) {
        Some(Event::Continued) => {}
        other => panic!("the confirmation of the continue was expected, not {other:?}"),
    }
}

/// The remote's thread pauses the running program after a while, waits
/// for the stop and terminates it.
fn pause_and_terminate(remote: DebugRemote) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        remote.pause();
        expect_paused(&remote);
        remote.send(Command::Terminate);
    })
}

#[test]
fn a_pause_before_the_run_stops_it_and_continue_finishes_it() {
    let (mut interpreter, instance) = instance_of(COUNTING);
    let remote = interpreter.attach_debugger();
    remote.pause();
    let agent = thread::spawn(move || {
        expect_paused(&remote);
        continue_stopped(&remote);
    });
    let count = expect_ok(run(&mut interpreter, &instance), "run");
    agent.join().expect("the remote's thread");
    assert_eq!(small_integer(&mut interpreter, &count), 1_000_000);
}

#[test]
fn a_pause_from_another_thread_stops_a_running_loop() {
    let (mut interpreter, instance) = instance_of(FOREVER);
    let agent = pause_and_terminate(interpreter.attach_debugger());
    let result = run(&mut interpreter, &instance);
    agent.join().expect("the remote's thread");
    assert_terminated(result);
}

#[test]
fn terminate_runs_the_pending_ensure_blocks() {
    let (mut interpreter, instance) = instance_of(ENSURING);
    let agent = pause_and_terminate(interpreter.attach_debugger());
    let result = run(&mut interpreter, &instance);
    agent.join().expect("the remote's thread");
    assert_terminated(result);
    let ensured = expect_ok(
        interpreter.send(&instance, Symbol::intern("ensured"), &[]),
        "ensured",
    );
    let ensured = interpreter
        .enter(|mutator| matches!(mutator.fetch(&ensured).kind(), ValueKind::Boolean(true)));
    assert!(ensured, "the ensure: block ran");
}

#[test]
fn a_terminate_queued_while_running_ends_the_run() {
    let (mut interpreter, instance) = instance_of(FOREVER);
    let remote = interpreter.attach_debugger();
    let agent = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        remote.send(Command::Terminate);
        // Held until the run ended, so that the termination is not taken
        // for the remote's departure.
        remote
    });
    let result = run(&mut interpreter, &instance);
    drop(agent.join().expect("the remote's thread"));
    assert_terminated(result);
}

#[test]
fn a_dropped_remote_lets_the_program_finish() {
    let (mut interpreter, instance) = instance_of(COUNTING);
    drop(interpreter.attach_debugger());
    let count = expect_ok(run(&mut interpreter, &instance), "run");
    assert_eq!(small_integer(&mut interpreter, &count), 1_000_000);
}

#[test]
fn a_remote_dropped_while_stopped_lets_the_program_finish() {
    let (mut interpreter, instance) = instance_of(COUNTING);
    let remote = interpreter.attach_debugger();
    remote.pause();
    let agent = thread::spawn(move || {
        expect_paused(&remote);
        drop(remote);
    });
    let count = expect_ok(run(&mut interpreter, &instance), "run");
    agent.join().expect("the remote's thread");
    assert_eq!(small_integer(&mut interpreter, &count), 1_000_000);
}

/// The pause is served at the top of the safepoint, before a collection
/// still requested can make it leave early; the program continued goes on
/// allocating, through the collections, until a second pause stops it
/// again.
#[test]
fn a_pause_while_a_collection_is_requested_is_served() {
    let (mut interpreter, instance) = instance_of(ALLOCATING);
    let remote = interpreter.attach_debugger();
    let agent = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        remote.pause();
        expect_paused(&remote);
        continue_stopped(&remote);
        thread::sleep(Duration::from_millis(50));
        remote.pause();
        expect_paused(&remote);
        remote.send(Command::Terminate);
    });
    let result = run(&mut interpreter, &instance);
    agent.join().expect("the remote's thread");
    assert_terminated(result);
}

#[test]
fn a_stop_only_command_while_running_is_refused() {
    let (mut interpreter, instance) = instance_of(FOREVER);
    let remote = interpreter.attach_debugger();
    let agent = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        remote.send(Command::StackTrace);
        match remote.next_event(EVENT_TIMEOUT) {
            Some(Event::Refused { command, .. }) => assert_eq!(command, "StackTrace"),
            other => panic!("the refusal of the stack trace was expected, not {other:?}"),
        }
        remote.send(Command::Terminate);
        remote
    });
    let result = run(&mut interpreter, &instance);
    drop(agent.join().expect("the remote's thread"));
    assert_terminated(result);
}

/// A remote whose debugger a later attachment replaced arms nothing: its
/// pause would otherwise stop the run for the new remote, which never
/// asked. The new remote's thread continues any stop it sees, so that a
/// failure ends rather than hangs, and reports it.
#[test]
fn a_replaced_remote_s_pause_does_not_stop_the_run() {
    let (mut interpreter, instance) = instance_of(COUNTING);
    let replaced = interpreter.attach_debugger();
    let current = interpreter.attach_debugger();
    replaced.pause();
    let finished = Arc::new(AtomicBool::new(false));
    let agent = {
        let finished = Arc::clone(&finished);
        thread::spawn(move || {
            let mut stopped = false;
            while !finished.load(Ordering::SeqCst) {
                if let Some(Event::Stopped { .. }) = current.next_event(Duration::from_millis(10)) {
                    stopped = true;
                    current.send(Command::Continue);
                }
            }
            stopped
        })
    };
    let count = expect_ok(run(&mut interpreter, &instance), "run");
    finished.store(true, Ordering::SeqCst);
    let stopped = agent.join().expect("the remote's thread");
    assert!(!stopped, "the replaced remote's pause stopped the run");
    assert_eq!(small_integer(&mut interpreter, &count), 1_000_000);
    drop(replaced);
}

/// A run that stops three sends deep, in a loop that never ends, with an
/// `Array` among the arguments of the stopped method.
const STACKED: &str = indoc! {"
    class Stacked = Object new () (
      public run = (
        ^self outer: 5
      )
      outer: count = (
        | items |
        items:: Array new: 3.
        items at: 1 put: 'one'.
        ^self loop: count with: items
      )
      loop: count with: items = (
        [true] whileTrue: []
      )
    ) : ()
"};

/// Sends `command` to the stopped program and answers its reply.
fn request(remote: &DebugRemote, command: Command) -> Event {
    remote.send(command);
    remote
        .next_event(EVENT_TIMEOUT)
        .expect("the stopped program answers")
}

/// Runs `source`'s `run`, pauses it from another thread, answers what
/// `query` answers while it is stopped, and terminates it. The run is
/// terminated whatever `query` does, so that a failing assertion fails the
/// test rather than leaving the loop running.
fn while_stopped<T: Send + 'static>(
    source: &str,
    query: impl FnOnce(&DebugRemote) -> T + Send + 'static,
) -> T {
    while_stopped_after(Duration::from_millis(50), source, |_, _| {}, query).0
}

/// `while_stopped` with the pause delayed by `delay`, long enough for a
/// body to be compiled when the test is about machine code, and the
/// interpreter set up by `set_up` once the instance exists. Answers what
/// `query` answered with the JIT's statistics from before the run and after
/// it.
fn while_stopped_after<T: Send + 'static>(
    delay: Duration,
    source: &str,
    set_up: impl FnOnce(&mut Interpreter, &Root),
    query: impl FnOnce(&DebugRemote) -> T + Send + 'static,
) -> (T, JitStatistics, JitStatistics) {
    let (mut interpreter, instance) = instance_of(source);
    set_up(&mut interpreter, &instance);
    let before = interpreter.jit_statistics();
    let remote = interpreter.attach_debugger();
    let agent = thread::spawn(move || {
        thread::sleep(delay);
        remote.pause();
        let answer = panic::catch_unwind(AssertUnwindSafe(|| {
            expect_paused(&remote);
            query(&remote)
        }));
        remote.send(Command::Terminate);
        // Held until the run ended, so that the termination is not taken
        // for the remote's departure.
        (answer, remote)
    });
    let result = run(&mut interpreter, &instance);
    let (answer, remote) = agent.join().expect("the remote's thread");
    drop(remote);
    assert_terminated(result);
    let after = interpreter.jit_statistics();
    let answer = answer.unwrap_or_else(|failure| panic::resume_unwind(failure));
    (answer, before, after)
}

fn stack_trace(remote: &DebugRemote) -> Vec<FrameDescription> {
    match request(remote, Command::StackTrace) {
        Event::StackTrace(frames) => frames,
        other => panic!("a stack trace was expected, not {other:?}"),
    }
}

fn variables(remote: &DebugRemote, command: Command) -> Vec<Variable> {
    match request(remote, command) {
        Event::Variables(variables) => variables,
        other => panic!("variables were expected, not {other:?}"),
    }
}

fn method(selector: &str) -> FrameKind {
    FrameKind::Method {
        selector: selector.to_owned(),
    }
}

/// The index of the stopped `loop:with:` activation in the stack trace.
fn loop_frame(remote: &DebugRemote) -> usize {
    stack_trace(remote)
        .iter()
        .position(|frame| frame.kind == method("loop:with:"))
        .expect("the loop's method is on the stack")
}

/// The variables of the stopped `loop:with:` activation.
fn loop_variables(remote: &DebugRemote) -> Vec<Variable> {
    let frame = loop_frame(remote);
    variables(remote, Command::Variables { frame })
}

fn named<'variables>(variables: &'variables [Variable], name: &str) -> &'variables Variable {
    variables
        .iter()
        .find(|variable| variable.name == name)
        .unwrap_or_else(|| panic!("a variable named {name} was expected"))
}

#[test]
fn the_stack_names_the_stopped_methods() {
    let kinds: Vec<FrameKind> = while_stopped(STACKED, |remote| {
        stack_trace(remote)
            .into_iter()
            .map(|frame| frame.kind)
            .collect()
    });
    let innermost = kinds
        .iter()
        .position(|kind| *kind == method("loop:with:"))
        .expect("the loop's method is on the stack");
    assert_eq!(
        kinds[innermost..],
        [method("loop:with:"), method("outer:"), method("run")],
        "the methods, innermost first, down to the run's base"
    );
}

#[test]
fn a_frame_s_variables_show_its_arguments() {
    let variables = while_stopped(STACKED, loop_variables);
    assert_eq!(variables[0].name, "self");
    let count = named(&variables, "argument 1");
    assert_eq!(count.value.label, "5");
    let items = named(&variables, "argument 2");
    assert_eq!(items.value.class_name, "Array");
    assert_eq!(items.value.child_count, 3);
}

#[test]
fn an_array_s_children_are_its_elements_paged() {
    let children = while_stopped(STACKED, |remote| {
        let items = named(&loop_variables(remote), "argument 2").value.id;
        variables(
            remote,
            Command::Children {
                value: items,
                start: 0,
                count: 2,
            },
        )
    });
    let names: Vec<&str> = children.iter().map(|child| child.name.as_str()).collect();
    assert_eq!(names, ["[1]", "[2]"]);
    assert_eq!(children[0].value.label, "the String \"one\"");
    assert_eq!(children[0].value.class_name, "String");
    assert_eq!(children[1].value.label, "nil");
}

#[test]
fn a_value_id_does_not_outlive_its_stop() {
    let answer = while_stopped(STACKED, |remote| {
        let items = named(&loop_variables(remote), "argument 2").value.id;
        continue_stopped(remote);
        remote.pause();
        expect_paused(remote);
        // The second stop hands out as many values as the first did, so
        // that an id counted afresh from each stop's start would name
        // the same argument again.
        loop_variables(remote);
        request(
            remote,
            Command::Children {
                value: items,
                start: 0,
                count: 3,
            },
        )
    });
    match answer {
        Event::Refused { command, .. } => assert_eq!(command, "Children"),
        other => panic!("the refusal of a stale value id was expected, not {other:?}"),
    }
}

#[test]
fn a_frame_index_past_the_stack_is_refused() {
    let answer = while_stopped(STACKED, |remote| {
        let depth = stack_trace(remote).len();
        request(remote, Command::Variables { frame: depth })
    });
    match answer {
        Event::Refused { command, .. } => assert_eq!(command, "Variables"),
        other => panic!("the refusal of a frame past the stack was expected, not {other:?}"),
    }
}

/// Recursion that never ends, whose only polls once compiled are the
/// prologues' depth compare.
const RECURSING: &str = indoc! {"
    class Recursing = Object new () (
      public run = (
        [true] whileTrue: [self fib: 24]
      )
      fib: n = (
        n < 2 ifTrue: [^n].
        ^(self fib: n - 1) + (self fib: n - 2)
      )
    ) : ()
"};

/// Whether the JIT's counters mean what these tests assert: machine code
/// exists in this build, the environment leaves it on, and no stress mode
/// changes when it compiles and exits.
fn counters_are_exact() -> bool {
    cfg!(ns_jit)
        && std::env::var_os("NS_NEXT_NO_JIT").is_none()
        && !std::env::vars_os().any(|(name, value)| {
            name.to_string_lossy().starts_with("NS_NEXT_STRESS_")
                && !value.is_empty()
                && value != "0"
        })
}

/// A pause stops a program that spends its time in compiled recursion, and
/// the stack it reads has the recursion's frames. No optimization request
/// asks for a safepoint, so the stop is the pause's own, taken at a poll of
/// baseline machine code.
#[test]
fn a_pause_stops_compiled_recursion() {
    let (kinds, before, after) = while_stopped_after(
        Duration::from_millis(200),
        RECURSING,
        |interpreter, _| interpreter.set_optimization_budget(i64::MAX),
        |remote| {
            stack_trace(remote)
                .into_iter()
                .map(|frame| frame.kind)
                .collect::<Vec<FrameKind>>()
        },
    );
    let depth = kinds.iter().filter(|kind| **kind == method("fib:")).count();
    assert!(depth > 1, "fib: frames more than one deep, not {depth}");
    if counters_are_exact() {
        assert!(
            after.compiled_bodies > before.compiled_bodies,
            "no body compiled: {after:?}"
        );
        assert!(
            after.machine_entries > before.machine_entries,
            "machine code never entered: {after:?}"
        );
        assert!(
            after.pushes_refused + after.safepoint_exits
                > before.pushes_refused + before.safepoint_exits,
            "no poll of machine code found the pause: {before:?} {after:?}"
        );
    }
}

/// A method whose optimized copy exists before the paused run: `forever`
/// pushes `spin:` again and again, so that each push enters the copy and the
/// pause finds the loop's poll in optimized code. The loop's block is one
/// the copy eliminates.
const OPTIMIZED: &str = indoc! {"
    class Optimized = Object new () (
      public run = (
        [true] whileTrue: [self spin: 100000000]
      )
      public warm = (
        ^self spin: 100
      )
      spin: limit = (
        | count |
        count:: 0.
        [count < limit] whileTrue: [count:: count + 1].
        ^count
      )
    ) : ()
"};

/// A stop at a poll of optimized code is deoptimized before the stack is
/// read, and the program, continued, goes on and stops again.
#[test]
fn a_stop_at_an_optimized_poll_is_deoptimized() {
    let ((), before, after) = while_stopped_after(
        Duration::from_millis(100),
        OPTIMIZED,
        |interpreter, instance| {
            interpreter.set_jit_stress(true);
            interpreter.set_optimize_stress(true);
            for _ in 0..6 {
                expect_ok(
                    interpreter.send(instance, Symbol::intern("warm"), &[]),
                    "warm",
                );
            }
        },
        |remote| {
            let frames = stack_trace(remote);
            let frame = frames
                .iter()
                .position(|frame| frame.kind == method("spin:"))
                .expect("the loop's method is on the stack");
            variables(remote, Command::Variables { frame });
            continue_stopped(remote);
            remote.pause();
            expect_paused(remote);
            stack_trace(remote);
        },
    );
    if counters_are_exact() {
        assert!(
            after.poll_deoptimizations > before.poll_deoptimizations,
            "no stop was deoptimized: {before:?} {after:?}"
        );
    }
}
