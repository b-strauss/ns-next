//! The debugging API at the run loop's safepoint: a host attaches a
//! debugger, pauses a running program from another thread, and continues
//! or terminates it, and reads its stack and values while it is stopped; a
//! command queued while the program runs is served at the next safepoint
//! (`docs/design/debugger.md`, decisions 7 and 9, "Commands while the
//! program runs", "Reading the stack at a stop", "Values the IDE holds").

use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use indoc::indoc;
use ns_runtime::debugger::FrameKind;
use ns_runtime::debugger::{
    BreakpointId, BreakpointState, ClassName, HitCondition, LineBreakpoint, MethodBreakpoint,
    MethodBreakpointState, ModuleTextAnswer,
};
use ns_runtime::JitStatistics;
use ns_runtime::{
    Command, DebugRemote, Event, FrameDescription, HostResult, HostSignal, Interpreter, Lowering,
    MethodSide, Root, StopReason, Symbol, ValueDescription, ValueKind, Variable, VariableValue,
};

/// How long the remote's thread waits for an event before it fails the
/// test: long enough for a debug build under a loaded machine.
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

/// The message of the error a run the debugger terminated ends with.
const TERMINATED: &str = "the debugger terminated the program";

/// An agent commands the VM from several threads, each through its own
/// clone of the control, and hears it on another: the halves must stay
/// so, or the agent no longer compiles for a reason its own code hides.
const _: () = {
    const fn shareable<T: Send + Sync + Clone>() {}
    const fn movable<T: Send>() {}
    shareable::<ns_runtime::DebugControl>();
    movable::<ns_runtime::DebugEvents>();
};

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

/// An interpreter compiling with `lowering`, with `source` loaded, and
/// its module class.
fn load(source: &str, lowering: Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
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

/// An instance of `source`'s module class under `lowering`, ready for its
/// `run`.
fn instance_of(source: &str, lowering: Lowering) -> (Interpreter, Root) {
    let (mut interpreter, class) = load(source, lowering);
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
            returned: None,
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
    let (mut interpreter, instance) = instance_of(COUNTING, Lowering::Optimized);
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

/// Two methods a host sends: one beginning with a send of its own, and one
/// whose local a block assigns, which lives in an indirection vector its
/// body makes before its first statement.
const ENTRY: &str = indoc! {"
    class Entry = Object new () (
      helper = (
        ^41
      )
      public plain = (
        self helper.
        ^self helper + 1
      )
      public captured = (
        | total ::= 0. |
        {1. 2} do: [:each | total:: total + each].
        ^total
      )
    ) : ()
"};

/// A pause asked between runs stops the next run's first activation, the
/// method the host sent, at its first statement start, before it sends
/// anything -- not at the run's first safepoint, the first push it makes
/// -- and reports the stop as the pause's, under either lowering.
#[test]
fn a_pause_between_runs_stops_the_sent_method_at_its_first_statement() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        for (selector, first_statement, answer) in [
            ("plain", "self helper.", 42),
            ("captured", "total ::= 0.", 3),
        ] {
            let (mut interpreter, instance) = instance_of(ENTRY, lowering);
            let remote = interpreter.attach_debugger();
            remote.pause();
            let agent = thread::spawn(move || {
                expect_paused(&remote);
                let frames = stack_trace(&remote);
                continue_stopped(&remote);
                frames
            });
            let result = interpreter.send(&instance, Symbol::intern(selector), &[]);
            let answered = expect_ok(result, selector);
            let frames = agent.join().expect("the remote's thread");
            assert_eq!(small_integer(&mut interpreter, &answered), answer);
            assert_eq!(frames.len(), 1, "{selector}: {frames:?}");
            assert_eq!(frames[0].name, format!("Entry>>{selector}"));
            let statement = frames[0]
                .location
                .as_ref()
                .unwrap_or_else(|| panic!("{selector}: the frame is placed"))
                .statement;
            let expected = ENTRY.find(first_statement).expect("the statement");
            assert_eq!(
                usize::try_from(statement.start).expect("a small offset"),
                expected,
                "{selector} under {lowering:?}"
            );
        }
    }
}

/// The two halves of a remote serve from two threads: one pumps the events
/// into a channel while the test thread pauses and continues through the
/// control, a clone of it for the second command.
#[test]
fn a_remote_split_in_two_serves_from_two_threads() {
    let (mut interpreter, instance) = instance_of(COUNTING, Lowering::Optimized);
    let (control, events) = interpreter.attach_debugger().into_parts();
    let (forwarded, received) = mpsc::channel();
    let pump = thread::spawn(move || {
        while let Some(event) = events.recv() {
            if forwarded.send(event).is_err() {
                break;
            }
        }
    });
    control.pause();
    let continuing = control.clone();
    let agent = thread::spawn(move || {
        match received.recv_timeout(EVENT_TIMEOUT) {
            Ok(Event::Stopped {
                reason: StopReason::Pause,
                returned: None,
            }) => {}
            other => panic!("a stop for the pause was expected, not {other:?}"),
        }
        continuing.send(Command::Continue);
        match received.recv_timeout(EVENT_TIMEOUT) {
            Ok(Event::Continued) => {}
            other => panic!("the confirmation of the continue was expected, not {other:?}"),
        }
        // Held until the run ended, so that its end is not taken for the
        // control's departure.
        continuing
    });
    let count = expect_ok(run(&mut interpreter, &instance), "run");
    let control = agent.join().expect("the agent's thread");
    assert_eq!(small_integer(&mut interpreter, &count), 1_000_000);
    drop((control, interpreter));
    pump.join().expect("the pumping thread");
}

#[test]
fn a_pause_from_another_thread_stops_a_running_loop() {
    let (mut interpreter, instance) = instance_of(FOREVER, Lowering::Optimized);
    let agent = pause_and_terminate(interpreter.attach_debugger());
    let result = run(&mut interpreter, &instance);
    agent.join().expect("the remote's thread");
    assert_terminated(result);
}

#[test]
fn terminate_runs_the_pending_ensure_blocks() {
    let (mut interpreter, instance) = instance_of(ENSURING, Lowering::Optimized);
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
    let (mut interpreter, instance) = instance_of(FOREVER, Lowering::Optimized);
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
    let (mut interpreter, instance) = instance_of(COUNTING, Lowering::Optimized);
    drop(interpreter.attach_debugger());
    let count = expect_ok(run(&mut interpreter, &instance), "run");
    assert_eq!(small_integer(&mut interpreter, &count), 1_000_000);
}

#[test]
fn a_remote_dropped_while_stopped_lets_the_program_finish() {
    let (mut interpreter, instance) = instance_of(COUNTING, Lowering::Optimized);
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
    let (mut interpreter, instance) = instance_of(ALLOCATING, Lowering::Optimized);
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
    let (mut interpreter, instance) = instance_of(FOREVER, Lowering::Optimized);
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
    let (mut interpreter, instance) = instance_of(COUNTING, Lowering::Optimized);
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

/// A pause the earlier remote asked before the replacement goes with its
/// debugger: the new remote meets no stop it never asked for. Its thread
/// continues any stop it sees, so that a failure ends rather than hangs.
#[test]
fn a_pause_before_a_replacement_does_not_stop_the_run() {
    let (mut interpreter, instance) = instance_of(COUNTING, Lowering::Optimized);
    let replaced = interpreter.attach_debugger();
    replaced.pause();
    let current = interpreter.attach_debugger();
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
    assert!(!stopped, "the earlier remote's pause stopped the run");
    assert_eq!(small_integer(&mut interpreter, &count), 1_000_000);
    drop(replaced);
}

/// A loop that ends, counting in a local so that every run counts afresh
/// and reaches safepoints, and one that never ends.
const RUNS: &str = indoc! {"
    class Runs = Object new () (
      public run = (
        | count |
        count:: 0.
        [count < 100000] whileTrue: [count:: count + 1].
        ^count
      )
      public forever = (
        [true] whileTrue: []
      )
    ) : ()
"};

/// A `Terminate` sent after its run ended is served by no run: the next
/// one, on the same interpreter and remote, finishes.
#[test]
fn a_terminate_after_its_run_ended_does_not_end_the_next() {
    let (mut interpreter, instance) = instance_of(RUNS, Lowering::Optimized);
    let remote = interpreter.attach_debugger();
    let first = expect_ok(run(&mut interpreter, &instance), "the first run");
    assert_eq!(small_integer(&mut interpreter, &first), 100_000);
    remote.send(Command::Terminate);
    let second = expect_ok(run(&mut interpreter, &instance), "the second run");
    assert_eq!(small_integer(&mut interpreter, &second), 100_000);
    drop(remote);
}

/// The commands queued after the `Terminate` that ended a run go with that
/// run, while a pause asked between runs stays: the next run stops for
/// the pause, and the stop takes the agent's `Continue`, not a stale
/// `Terminate`, so the run finishes.
#[test]
fn a_terminate_queued_after_the_one_that_ended_the_run_ends_no_other() {
    let (mut interpreter, instance) = instance_of(RUNS, Lowering::Optimized);
    let remote = interpreter.attach_debugger();
    let agent = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        remote.send(Command::Terminate);
        remote.send(Command::Terminate);
        remote
    });
    let result = interpreter.send(&instance, Symbol::intern("forever"), &[]);
    let remote = agent.join().expect("the remote's thread");
    assert_terminated(result);
    remote.pause();
    let agent = thread::spawn(move || {
        // The dropped `Terminate` is answered before the next run stops.
        match remote.next_event(EVENT_TIMEOUT) {
            Some(Event::Refused { command, .. }) => assert_eq!(command, "Terminate"),
            other => panic!("the refusal of the dropped terminate was expected, not {other:?}"),
        }
        expect_paused(&remote);
        continue_stopped(&remote);
        remote
    });
    let count = expect_ok(run(&mut interpreter, &instance), "the next run");
    drop(agent.join().expect("the remote's thread"));
    assert_eq!(small_integer(&mut interpreter, &count), 100_000);
}

/// Every command queued while no run executes has one answer: the drop
/// of the ones no run serves is announced by a refusal, in the order the
/// commands were sent, and the run that follows is not affected.
#[test]
fn a_command_queued_between_runs_is_refused() {
    let (mut interpreter, instance) = instance_of(RUNS, Lowering::Optimized);
    let remote = interpreter.attach_debugger();
    remote.send(Command::StackTrace);
    remote.send(Command::Terminate);
    let count = expect_ok(run(&mut interpreter, &instance), "the run");
    assert_eq!(small_integer(&mut interpreter, &count), 100_000);
    for expected in ["StackTrace", "Terminate"] {
        match remote.next_event(EVENT_TIMEOUT) {
            Some(Event::Refused { command, reason }) => {
                assert_eq!(command, expected);
                assert_eq!(reason, "no run executes");
            }
            other => panic!("the refusal of {expected} was expected, not {other:?}"),
        }
    }
    drop(remote);
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

/// How long a test waits before it pauses the run, when the stop need not
/// find compiled code.
const PAUSE_DELAY: Duration = Duration::from_millis(50);

/// Runs `source`'s `run`, pauses it from another thread, answers what
/// `query` answers while it is stopped, and terminates it. The run is
/// terminated whatever `query` does, so that a failing assertion fails the
/// test rather than leaving the loop running.
fn while_stopped<T: Send + 'static>(
    source: &str,
    query: impl FnOnce(&DebugRemote) -> T + Send + 'static,
) -> T {
    while_stopped_under(Lowering::Optimized, source, query)
}

/// `while_stopped` with `source` compiled under `lowering`.
fn while_stopped_under<T: Send + 'static>(
    lowering: Lowering,
    source: &str,
    query: impl FnOnce(&DebugRemote) -> T + Send + 'static,
) -> T {
    let (interpreter, instance) = instance_of(source, lowering);
    while_stopped_in(interpreter, &instance, PAUSE_DELAY, query).0
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
    let (mut interpreter, instance) = instance_of(source, Lowering::Optimized);
    set_up(&mut interpreter, &instance);
    while_stopped_in(interpreter, &instance, delay, query)
}

/// Sends `run` to `instance` of `interpreter`, pauses it after `delay`
/// from another thread, answers what `query` answers while it is stopped
/// and terminates it, with the JIT's statistics from before the run and
/// after it.
fn while_stopped_in<T: Send + 'static>(
    mut interpreter: Interpreter,
    instance: &Root,
    delay: Duration,
    query: impl FnOnce(&DebugRemote) -> T + Send + 'static,
) -> (T, JitStatistics, JitStatistics) {
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
    let result = run(&mut interpreter, instance);
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

/// The value of `variable`, which must be available.
fn shown(variable: &Variable) -> &ValueDescription {
    variable
        .value
        .shown()
        .unwrap_or_else(|| panic!("{} was expected to be available", variable.name))
}

/// The value of the variable named `name`, which must be available.
fn value_of<'variables>(
    variables: &'variables [Variable],
    name: &str,
) -> &'variables ValueDescription {
    shown(named(variables, name))
}

/// The variables' names, in order.
fn names(variables: &[Variable]) -> Vec<&str> {
    variables
        .iter()
        .map(|variable| variable.name.as_str())
        .collect()
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

/// Under the suspend stress mode every safepoint suspends the run's
/// activations into their frame objects and resumes only the top one, so
/// that at the stop `loop:with:` runs on the activation stack and its
/// senders are suspended: the trace reaches them, and a suspended frame's
/// variables are read from its frame object.
#[cfg(debug_assertions)]
#[test]
fn the_stack_reaches_suspended_senders() {
    let ((kinds, outer), _, _) = while_stopped_after(
        Duration::from_millis(50),
        STACKED,
        |interpreter, _| interpreter.set_stress_suspend(true),
        |remote| {
            let kinds: Vec<FrameKind> = stack_trace(remote)
                .into_iter()
                .map(|frame| frame.kind)
                .collect();
            let frame = kinds
                .iter()
                .position(|kind| *kind == method("outer:"))
                .expect("the suspended sender is on the stack");
            (kinds, variables(remote, Command::Variables { frame }))
        },
    );
    assert_eq!(
        kinds,
        [method("loop:with:"), method("outer:"), method("run")],
        "the running activation, then its suspended senders down to the run's base"
    );
    assert_eq!(names(&outer), ["self", "count", "items"]);
    assert_eq!(value_of(&outer, "count").label, "5");
    let items = value_of(&outer, "items");
    assert_eq!(items.class_name, "Array");
    assert_eq!(items.child_count, 3);
}

#[test]
fn an_array_s_children_are_its_elements_paged() {
    let (items, children) = while_stopped(STACKED, |remote| {
        let items = value_of(&loop_variables(remote), "items").clone();
        let children = variables(
            remote,
            Command::Children {
                value: items.id,
                start: 0,
                count: 2,
            },
        );
        (items, children)
    });
    assert!(items.indexed, "an Array's children are its elements");
    let names: Vec<&str> = children.iter().map(|child| child.name.as_str()).collect();
    assert_eq!(names, ["[1]", "[2]"]);
    assert_eq!(shown(&children[0]).label, "the String \"one\"");
    assert_eq!(shown(&children[0]).class_name, "String");
    assert_eq!(shown(&children[1]).label, "nil");
}

#[test]
fn an_instance_s_children_are_its_fields() {
    let (receiver, children) = while_stopped(FOREVER, |remote| {
        let frame = stack_trace(remote)
            .iter()
            .position(|frame| frame.kind == method("run"))
            .expect("the loop's method is on the stack");
        let receiver = value_of(&variables(remote, Command::Variables { frame }), "self").clone();
        let children = variables(
            remote,
            Command::Children {
                value: receiver.id,
                start: 0,
                count: 8,
            },
        );
        (receiver, children)
    });
    assert_eq!(receiver.class_name, "Forever");
    assert_eq!(receiver.child_count, 1);
    assert!(!receiver.indexed, "an instance's children are its fields");
    let names: Vec<&str> = children.iter().map(|child| child.name.as_str()).collect();
    assert_eq!(names, ["count"]);
    assert!(
        shown(&children[0]).label.parse::<u64>().is_ok(),
        "the count slot holds a number, not {}",
        shown(&children[0]).label
    );
}

/// An instance with slots, nested classes -- one never used -- a subclass
/// whose private slot shadows its superclass's, a class applying one
/// mixin three times, and the platform, whose `transcript` it used. Its
/// `run` stops at the breakpoint on its last statement, where every one
/// of them is built: its fields are named by the slots and nested
/// classes that declare them.
const FIELDS: &str = indoc! {"
    class Fields = Object new (
      | n ::= 50000. sum ::= 0. derived thrice platform |
    ) (
      class KindA = Object new () ()
      class Base = Object new (
        | private x = 1. y = 2. |
      ) ()
      class Derived = Base new (
        | private x = 3. z = 4. |
      ) ()
      class Unused = Object new () ()
      class M = Object new (
        | x = 5. |
      ) ()
      class Thrice = Object new <: M new <: M new <: M new (
        | w = 6. |
      ) ()
      public use: aPlatform = (
        platform:: aPlatform
      )
      public run = (
        KindA new.
        derived:: Derived new.
        thrice:: Thrice new.
        platform transcript.
        sum:: sum + 1.
        ^sum
      )
    ) : ()
"};

/// The fields `fields_while_stopped` reads at the breakpoint.
struct StoppedFields {
    /// `self`'s fields from the `start` asked, at most `count` of them.
    page: Vec<Variable>,
    derived: Vec<Variable>,
    thrice: Vec<Variable>,
    platform: Vec<Variable>,
}

/// `FIELDS` under `lowering`, handed the platform and stopped at the
/// breakpoint on `run`'s last statement: the fields it shows there.
fn fields_while_stopped(lowering: Lowering, start: usize, count: usize) -> StoppedFields {
    let (mut interpreter, instance) = instance_of(FIELDS, lowering);
    let platform = interpreter.platform();
    expect_ok(
        interpreter.send(&instance, Symbol::intern("use:"), &[platform]),
        "use:",
    );
    let offset = line_start(FIELDS, "sum:: sum + 1.");
    let (fields, result) = debugged_in(
        &mut interpreter,
        &instance,
        |remote| set_breakpoints(remote, "Fields", vec![offset]),
        move |remote| {
            let (id, _) = resolved(&expect_breakpoints(remote, "Fields"));
            assert_eq!(expect_breakpoint_stop(remote), [id]);
            let frame = stack_trace(remote)
                .iter()
                .position(|frame| frame.kind == method("run"))
                .expect("the stopped method is on the stack");
            let receiver =
                value_of(&variables(remote, Command::Variables { frame }), "self").clone();
            let all = variables(
                remote,
                Command::Children {
                    value: receiver.id,
                    start: 0,
                    count: receiver.child_count,
                },
            );
            let children_of = |name: &str| {
                let value = value_of(&all, name);
                variables(
                    remote,
                    Command::Children {
                        value: value.id,
                        start: 0,
                        count: value.child_count,
                    },
                )
            };
            let fields = StoppedFields {
                page: variables(
                    remote,
                    Command::Children {
                        value: receiver.id,
                        start,
                        count,
                    },
                ),
                derived: children_of("derived"),
                thrice: children_of("thrice"),
                platform: children_of("platform"),
            };
            continue_stopped(remote);
            fields
        },
    );
    expect_ok(result, "run");
    fields
}

#[test]
fn an_instance_s_fields_are_named_by_its_slots_and_nested_classes() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let fields = fields_while_stopped(lowering, 0, 16).page;
        assert_eq!(
            names(&fields),
            [
                "n", "sum", "derived", "thrice", "platform", "KindA", "Base", "Derived", "Unused",
                "M", "Thrice"
            ],
            "{lowering:?}"
        );
        assert_eq!(value_of(&fields, "n").label, "50000", "{lowering:?}");
        assert_eq!(
            value_of(&fields, "KindA").label,
            "the class KindA",
            "{lowering:?}"
        );
        assert_eq!(
            named(&fields, "Unused").value,
            VariableValue::NotComputed,
            "{lowering:?}: a nested class never used is not computed yet"
        );
    }
}

#[test]
fn an_instance_s_fields_include_its_superclass_s_and_name_a_shadowed_one() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let derived = fields_while_stopped(lowering, 0, 0).derived;
        assert_eq!(names(&derived), ["x (Base)", "y", "x", "z"], "{lowering:?}");
        let labels: Vec<&str> = derived
            .iter()
            .map(|field| shown(field).label.as_str())
            .collect();
        assert_eq!(labels, ["1", "2", "3", "4"], "{lowering:?}");
    }
}

/// Each `<:` clause adds a level, and a chain applying one mixin three
/// times has three levels declaring its slot: the nearest is the plain
/// name, the ones above it numbered up from it.
#[test]
fn a_mixin_applied_more_than_once_numbers_its_shadowed_slots() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let thrice = fields_while_stopped(lowering, 0, 0).thrice;
        assert_eq!(
            names(&thrice),
            ["x (M 2)", "x (M)", "x", "w"],
            "{lowering:?}"
        );
        let labels: Vec<&str> = thrice
            .iter()
            .map(|field| shown(field).label.as_str())
            .collect();
        assert_eq!(labels, ["5", "5", "5", "6"], "{lowering:?}");
    }
}

/// Two different nested classes named `Base`, each declaring a slot `x`,
/// both levels of one chain -- the superclass and a `<:` clause's mixin --
/// under a class that declares `x` too.
const TWINS: &str = indoc! {"
    class Twins = Object new () (
      class Outer = Object new () (
        public class Base = Object new (
          | x = 1. |
        ) ()
      )
      class Other = Object new () (
        public class Base = Object new (
          | x = 2. |
        ) ()
      )
      class Both = outerBase new <: otherBase new (
        | x = 3. |
      ) ()
      outerBase = (
        ^Outer new Base
      )
      otherBase = (
        ^Other new Base
      )
      public run = (
        | both |
        both:: Both new.
        ^both
      )
    ) : ()
"};

/// Two levels of one chain whose mixins differ but share a name would
/// both read `x (Base)`: each is named by its declaration's path from
/// its module root instead, and only where the short names collide.
#[test]
fn two_shadowed_slots_of_mixins_sharing_a_name_are_named_by_their_paths() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let offset = line_start(TWINS, "^both");
        let (fields, result) = debugged(
            TWINS,
            lowering,
            |remote| set_breakpoints(remote, "Twins", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Twins");
                let both =
                    value_of(&variables(remote, Command::Variables { frame: 0 }), "both").clone();
                let fields = variables(
                    remote,
                    Command::Children {
                        value: both.id,
                        start: 0,
                        count: both.child_count,
                    },
                );
                continue_stopped(remote);
                fields
            },
        );
        expect_ok(result, "run");
        assert_eq!(
            names(&fields),
            ["x (Outer.Base)", "x (Other.Base)", "x"],
            "{lowering:?}"
        );
        let labels: Vec<&str> = fields
            .iter()
            .map(|field| shown(field).label.as_str())
            .collect();
        assert_eq!(labels, ["1", "2", "3"], "{lowering:?}");
    }
}

/// A module whose `Outer.Base` declares `x`, which `JOINED` applies.
const LIBRARY: &str = indoc! {"
    class Library = Object new () (
      public class Outer = Object new () (
        public class Base = Object new (
          | x = 1. |
        ) ()
      )
    ) : ()
"};

/// A module whose class `Both` applies the `Outer.Base` of each of the two
/// modules its factory takes, and declares `x` itself.
const JOINED: &str = indoc! {"
    class Joined first: first second: second = Object new (
      |
        firstLibrary = first.
        secondLibrary = second.
      |
    ) (
      class Both = firstBase new <: secondBase new (
        | x = 3. |
      ) ()
      firstBase = (
        ^firstLibrary Outer new Base
      )
      secondBase = (
        ^secondLibrary Outer new Base
      )
      public run = (
        | both |
        both:: Both new.
        ^both
      )
    ) : ()
"};

/// A module whose class `Both` applies an `Outer.Base` of each of the
/// three modules its factory takes -- the first's nested one level deeper,
/// in its `Library` -- and declares `x` itself.
const JOINED_THREE: &str = indoc! {"
    class Joined first: first second: second third: third = Object new (
      |
        firstShelf = first.
        secondLibrary = second.
        thirdLibrary = third.
      |
    ) (
      class Both = firstBase new <: secondBase new <: thirdBase new (
        | x = 4. |
      ) ()
      firstBase = (
        ^firstShelf Library new Outer new Base
      )
      secondBase = (
        ^secondLibrary Outer new Base
      )
      thirdBase = (
        ^thirdLibrary Outer new Base
      )
      public run = (
        | both |
        both:: Both new.
        ^both
      )
    ) : ()
"};

/// A module whose `Library.Outer.Base` declares `x`: its path from its
/// root spells what `LIBRARY`'s `Outer.Base` is named from its root's name.
const SHELF: &str = indoc! {"
    class Shelf = Object new () (
      public class Library = Object new () (
        public class Outer = Object new () (
          public class Base = Object new (
            | x = 1. |
          ) ()
        )
      )
    ) : ()
"};

/// The names and values of the fields of `JOINED`'s `Both` under
/// `lowering`, its two levels applied from instances of the modules
/// `first` and `second`.
fn joined_fields(lowering: Lowering, first: &str, second: &str) -> (Vec<String>, Vec<String>) {
    joined_fields_of(lowering, JOINED, &[first, second])
}

/// The names and values of the fields of the `Both` of `joined`, a module
/// whose factory takes an instance of each of `libraries`, under
/// `lowering`.
fn joined_fields_of(
    lowering: Lowering,
    joined: &'static str,
    libraries: &[&str],
) -> (Vec<String>, Vec<String>) {
    let source = joined;
    let (mut interpreter, joined) = interpreted(source, lowering);
    let mut library = |source: &str| {
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = ns_runtime::resolve(
            module,
            source.into(),
            ns_runtime::SourceOrigin::File("library.ns".into()),
        )
        .expect("resolve");
        let class = interpreter.load_module(&resolved);
        expect_ok(
            interpreter.instantiate(&class, &[]),
            "instantiate a library",
        )
    };
    let instances: Vec<Root> = libraries.iter().map(|source| library(source)).collect();
    let instance = expect_ok(interpreter.instantiate(&joined, &instances), "instantiate");
    let offset = line_start(source, "^both");
    let (fields, result) = debugged_in(
        &mut interpreter,
        &instance,
        |remote| set_breakpoints(remote, "Joined", vec![offset]),
        |remote| {
            expect_first_stop(remote, "Joined");
            let both =
                value_of(&variables(remote, Command::Variables { frame: 0 }), "both").clone();
            let fields = variables(
                remote,
                Command::Children {
                    value: both.id,
                    start: 0,
                    count: both.child_count,
                },
            );
            continue_stopped(remote);
            fields
        },
    );
    expect_ok(result, "run");
    (
        names(&fields).into_iter().map(String::from).collect(),
        fields
            .iter()
            .map(|field| shown(field).label.clone())
            .collect(),
    )
}

/// Two shadowed levels whose mixins share their name and their path from
/// their module roots, the roots of two modules of different names, are
/// named by the path from the root's own name on.
#[test]
fn shadowed_slots_of_modules_sharing_a_path_are_named_from_their_roots() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let kit = LIBRARY.replace("Library", "Kit").replace("x = 1", "x = 2");
        let (names, labels) = joined_fields(lowering, LIBRARY, &kit);
        assert_eq!(
            names,
            ["x (Library.Outer.Base)", "x (Kit.Outer.Base)", "x"],
            "{lowering:?}"
        );
        assert_eq!(labels, ["1", "2", "3"], "{lowering:?}");
    }
}

/// Two shadowed levels whose mixins share even the path from their roots'
/// names, those of two modules of one name, are told apart by their
/// places in the chain, numbered from the object's class upward.
#[test]
fn shadowed_slots_of_modules_sharing_a_name_and_a_path_are_numbered() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let second = LIBRARY.replace("x = 1", "x = 2");
        let (names, labels) = joined_fields(lowering, LIBRARY, &second);
        assert_eq!(
            names,
            [
                "x (Library.Outer.Base #2)",
                "x (Library.Outer.Base #1)",
                "x"
            ],
            "{lowering:?}"
        );
        assert_eq!(labels, ["1", "2", "3"], "{lowering:?}");
    }
}

/// A declarer named by its path from its module root and another named by
/// the path from its root's own name may spell the same label: each is
/// numbered then, as declarers alike in every tier are.
#[test]
fn shadowed_slots_alike_across_their_tiers_are_numbered() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let second = LIBRARY.replace("x = 1", "x = 2");
        let third = LIBRARY.replace("Library", "Kit").replace("x = 1", "x = 3");
        let (names, labels) = joined_fields_of(lowering, JOINED_THREE, &[SHELF, &second, &third]);
        assert_eq!(
            names,
            [
                "x (Library.Outer.Base #2)",
                "x (Library.Outer.Base #1)",
                "x (Kit.Outer.Base)",
                "x"
            ],
            "{lowering:?}"
        );
        assert_eq!(labels, ["1", "2", "3", "4"], "{lowering:?}");
    }
}

#[test]
fn named_fields_are_paged() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let page = fields_while_stopped(lowering, 1, 2).page;
        assert_eq!(names(&page), ["sum", "derived"], "{lowering:?}");
        let past = fields_while_stopped(lowering, 9, 5).page;
        assert_eq!(names(&past), ["M", "Thrice"], "{lowering:?}");
    }
}

/// The platform is a shipped class's instance, named from the shipped
/// mixin's slots; its facets are lazy slots, of which only the one the
/// program used holds a value, and the others are not computed yet --
/// not `nil`, which none of them would answer.
#[test]
fn a_lazy_slot_not_computed_yet_is_shown_so() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let platform = fields_while_stopped(lowering, 0, 0).platform;
        assert_eq!(
            names(&platform),
            [
                "transcript",
                "collections",
                "kernel",
                "time",
                "typedArrays",
                "mirrors"
            ],
            "{lowering:?}"
        );
        let transcript = named(&platform, "transcript");
        assert_eq!(shown(transcript).class_name, "Transcript", "{lowering:?}");
        for facet in &platform[1..] {
            assert_eq!(
                facet.value,
                VariableValue::NotComputed,
                "{lowering:?}: {} is not computed yet",
                facet.name
            );
        }
    }
}

#[test]
fn a_value_id_does_not_outlive_its_stop() {
    let answer = while_stopped(STACKED, |remote| {
        let items = value_of(&loop_variables(remote), "items").id;
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

/// How many times `a_pause_stops_compiled_recursion` pauses again after a
/// stop between two turns of `run`'s loop. Under the collect stress mode on
/// a loaded machine a stop lands there about once in twenty, so that fifty
/// in a row never happen by chance, while a pause that never reaches the
/// recursion still fails.
const PAUSES_UNTIL_IN_RECURSION: usize = 50;

/// Whether a stop that stands at `trace` came between two turns of
/// `RECURSING`'s `run` loop rather than in its recursion: in `run` itself,
/// or at the first instruction of the `fib:` it sends, nothing below it.
fn stopped_between_turns(trace: &[FrameDescription]) -> bool {
    match trace {
        [run] => run.kind == method("run"),
        [fib, run] => {
            fib.kind == method("fib:") && fib.program_counter == 0 && run.kind == method("run")
        }
        _ => false,
    }
}

/// A pause stops a program that spends its time in compiled recursion, and
/// the stack it reads has the recursion's frames. No optimization request
/// asks for a safepoint, so the stop is the pause's own, taken at a poll of
/// baseline machine code.
///
/// Under a stress mode, every turn of `run`'s loop takes the safepoint at
/// its backward jump, and the stress mode's work there -- a full collection
/// under the collect stress mode -- widens the stretch between the
/// recursions, in which a pause stops `run` or its `fib: 24` at its first
/// instruction. Such a stop is a pause's correct stop, but not the one this
/// test is about, so the test continues it and pauses again.
#[test]
fn a_pause_stops_compiled_recursion() {
    let (kinds, before, after) = while_stopped_after(
        Duration::from_millis(200),
        RECURSING,
        |interpreter, _| interpreter.set_optimization_budget(i64::MAX),
        |remote| {
            let mut trace = stack_trace(remote);
            for _ in 0..PAUSES_UNTIL_IN_RECURSION {
                if !stopped_between_turns(&trace) {
                    break;
                }
                continue_stopped(remote);
                remote.pause();
                expect_paused(remote);
                trace = stack_trace(remote);
            }
            trace
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
/// pause finds the loop's poll in optimized code, which the stop
/// deoptimizes before the stack is read.
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

// Names, places and named variables from the debugger metadata
// (`docs/design/debugger.md`, "Reading the stack at a stop", decisions 5,
// 10, 12 and 13).

/// The frames' names, innermost first.
fn frame_names(frames: &[FrameDescription]) -> Vec<&str> {
    frames.iter().map(|frame| frame.name.as_str()).collect()
}

/// The index of the frame named `name` nearest the top.
fn frame_named(frames: &[FrameDescription], name: &str) -> usize {
    frames
        .iter()
        .position(|frame| frame.name == name)
        .unwrap_or_else(|| {
            panic!(
                "a frame named {name} was expected in {:?}",
                frame_names(frames)
            )
        })
}

/// The stack trace and the variables of the frame named `name` nearest
/// the top.
fn frame_and_variables(remote: &DebugRemote, name: &str) -> (Vec<FrameDescription>, Vec<Variable>) {
    let frames = stack_trace(remote);
    let frame = frame_named(&frames, name);
    let variables = variables(remote, Command::Variables { frame });
    (frames, variables)
}

ns_runtime::for_each_lowering! {
    /// Each frame is named for what it runs and located in its module's
    /// text: the statement running, and below the top the send it waits
    /// on. The reference lowering sends `value` to the loop's blocks, so
    /// that the stop may come in one of them, a block of `loop:with:`.
    fn a_frame_is_named_and_located(lowering) {
        let frames = while_stopped_under(lowering, STACKED, stack_trace);
        let names = frame_names(&frames);
        let innermost = frame_named(&frames, "Stacked>>loop:with:");
        assert_eq!(
            names[innermost..],
            ["Stacked>>loop:with:", "Stacked>>outer:", "Stacked>>run"]
        );
        for name in &names[..innermost] {
            assert_eq!(*name, "[] in Stacked>>loop:with:");
        }
        let text = |span: ns_syntax::Span| span.text(STACKED).to_owned();
        let located = |index: usize| {
            frames[index]
                .location
                .clone()
                .unwrap_or_else(|| panic!("{} is located", frames[index].name))
        };
        let looping = located(innermost);
        assert_eq!(looping.module, "Stacked");
        // The loop's statement, or its condition's, the statement of an
        // inlined block, which the loop's back edge jumps to.
        assert!(
            ["[true] whileTrue: []", "true"].contains(&text(looping.statement).as_str()),
            "{}",
            text(looping.statement)
        );
        if innermost == 0 {
            assert_eq!(looping.send, None, "the top frame waits on no send");
        }
        let outer = located(innermost + 1);
        assert_eq!(text(outer.statement), "^self loop: count with: items");
        let send = outer.send.expect("outer: waits on its send");
        assert_eq!(send.receiver.map(text).as_deref(), Some("self"));
        assert_eq!(
            send.selector.into_iter().map(text).collect::<Vec<_>>(),
            ["loop:", "with:"]
        );
        let running = located(innermost + 2);
        assert_eq!(text(running.statement), "^self outer: 5");
        let send = running.send.expect("run waits on its send");
        assert_eq!(send.receiver.map(text).as_deref(), Some("self"));
        assert_eq!(
            send.selector.into_iter().map(text).collect::<Vec<_>>(),
            ["outer:"]
        );
    }
}

ns_runtime::for_each_lowering! {
    /// A method's frame shows `self`, then its parameters and temporaries
    /// by name.
    fn a_frame_s_variables_are_named(lowering) {
        let (looping, outer) = while_stopped_under(lowering, STACKED, |remote| {
            let (_, looping) = frame_and_variables(remote, "Stacked>>loop:with:");
            let (_, outer) = frame_and_variables(remote, "Stacked>>outer:");
            (looping, outer)
        });
        for variables in [&looping, &outer] {
            assert_eq!(names(variables), ["self", "count", "items"]);
            assert_eq!(value_of(variables, "self").class_name, "Stacked");
            assert_eq!(value_of(variables, "count").label, "5");
            let items = value_of(variables, "items");
            assert_eq!(items.class_name, "Array");
            assert_eq!(items.child_count, 3);
        }
    }
}

/// A block evaluated while its method runs, stopped in a loop inside it.
const BLOCKED: &str = indoc! {"
    class Blocked = Object new () (
      public run = (
        ^self outer: 5
      )
      outer: count = (
        | items block |
        items:: Array new: 3.
        block:: [:each |
          | inner |
          inner:: each + count.
          [true] whileTrue: [inner:: inner + 0]
        ].
        ^block value: 7
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A block's frame shows the variables of its whole lexical chain: its
    /// own, and those of its method, whether the block captured them or
    /// not (decision 10).
    fn a_block_shows_its_method_s_variables(lowering) {
        let variables = while_stopped_under(lowering, BLOCKED, |remote| {
            frame_and_variables(remote, "[] in Blocked>>outer:").1
        });
        assert_eq!(variables[0].name, "self");
        for name in ["each", "inner", "count", "items", "block"] {
            named(&variables, name);
        }
        assert_eq!(value_of(&variables, "self").class_name, "Blocked");
        assert_eq!(value_of(&variables, "each").label, "7");
        assert_eq!(value_of(&variables, "inner").label, "12");
        assert_eq!(value_of(&variables, "count").label, "5");
        assert_eq!(value_of(&variables, "items").class_name, "Array");
    }
}

/// A block stored by a method that returned, evaluated afterwards and
/// stopped in a loop inside it. It reads `limit`, which the optimized
/// lowering copies into it and the reference lowering reads from the
/// method's frame, which therefore keeps its registers; `unused` it never
/// reads.
const RETURNED: &str = indoc! {"
    class Returned = Object new (
      | block |
    ) (
      public run = (
        self make: 3.
        ^block value
      )
      make: limit = (
        | unused |
        unused:: 42.
        block:: [
          | count |
          count:: limit.
          [true] whileTrue: [count:: count + 1]
        ]
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A variable of a returned method that the block did not capture
    /// lives nowhere once the method's frame dropped its registers, as the
    /// optimized lowering's does: it is shown as not available. The
    /// reference lowering's frame kept them, and shows it.
    fn an_uncaptured_variable_of_a_returned_creator_is_not_available(lowering) {
        let variables = while_stopped_under(lowering, RETURNED, |remote| {
            frame_and_variables(remote, "[] in Returned>>make:").1
        });
        assert_eq!(value_of(&variables, "limit").label, "3");
        let unused = named(&variables, "unused");
        match lowering {
            Lowering::Optimized => assert_eq!(unused.value, VariableValue::Unavailable),
            Lowering::Reference => assert_eq!(shown(unused).label, "42"),
        }
    }
}

/// A block two deep, stopped in a loop inside it while the method that
/// declares `unused` -- which no block reads -- still runs.
const DEEP_RUNNING: &str = indoc! {"
    class Deep = Object new () (
      public run = (
        ^self make: 3
      )
      make: limit = (
        | unused first |
        unused:: 42.
        first:: [
          | middle inner |
          middle:: limit.
          inner:: [
            | count |
            count:: middle.
            [true] whileTrue: [count:: count + 1]
          ].
          inner value
        ].
        ^first value
      )
    ) : ()
"};

/// The block two deep of `DEEP_RUNNING`, stored by a method that
/// returned and evaluated afterwards.
const DEEP_RETURNED: &str = indoc! {"
    class Deep = Object new (
      | block |
    ) (
      public run = (
        self make: 3.
        ^block value
      )
      make: limit = (
        | unused |
        unused:: 42.
        block:: [
          | middle inner |
          middle:: limit.
          inner:: [
            | count |
            count:: middle.
            [true] whileTrue: [count:: count + 1]
          ].
          inner value
        ]
      )
    ) : ()
"};

/// The variables of the block two deep of `source`'s `make:` -- the
/// frame named for `make:` nearest the top, since the reference lowering
/// sends `value` to the loop's blocks and stops in them, the one whose
/// own variable is `count`.
fn innermost_deep_block(lowering: Lowering, source: &'static str) -> Vec<Variable> {
    while_stopped_under(lowering, source, |remote| {
        let frames = stack_trace(remote);
        let mut found = None;
        for (frame, description) in frames.iter().enumerate() {
            if description.name != "[] in Deep>>make:" {
                continue;
            }
            let variables = variables(remote, Command::Variables { frame });
            if variables
                .get(1)
                .is_some_and(|variable| variable.name == "count")
            {
                found = Some(variables);
                break;
            }
        }
        found.unwrap_or_else(|| {
            panic!(
                "a frame of the inner block was expected in {:?}",
                frame_names(&frames)
            )
        })
    })
}

ns_runtime::for_each_lowering! {
    /// A block two deep reads a variable of its method that no block
    /// reads two captured frames out: while the method runs, from its
    /// activation; after it returned, from its frame object, which only
    /// the reference lowering's creation of the outer block, reading its
    /// method by reference, made keep its registers.
    fn a_block_two_deep_reads_its_method_s_variable_two_frames_out(lowering) {
        let running = innermost_deep_block(lowering, DEEP_RUNNING);
        assert_eq!(names(&running)[..4], ["self", "count", "middle", "inner"]);
        assert_eq!(value_of(&running, "middle").label, "3");
        assert_eq!(value_of(&running, "limit").label, "3");
        assert_eq!(value_of(&running, "unused").label, "42");
        assert_eq!(value_of(&running, "first").class_name, "Closure");

        let returned = innermost_deep_block(lowering, DEEP_RETURNED);
        assert_eq!(value_of(&returned, "middle").label, "3");
        assert_eq!(value_of(&returned, "limit").label, "3");
        let unused = named(&returned, "unused");
        match lowering {
            Lowering::Optimized => assert_eq!(unused.value, VariableValue::Unavailable),
            Lowering::Reference => assert_eq!(shown(unused).label, "42"),
        }
    }
}

/// An instance created through a factory whose initializer never ends.
const FACTORIES: &str = indoc! {"
    class Factories = Object new () (
      class Made x: first y: second = (
        | sum = self spin: first + second. |
      ) (
        spin: limit = (
          [true] whileTrue: [].
          ^limit
        )
      )
      public run = (
        ^Made x: 3 y: 4
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A factory's frame is named for its class side and its selector,
    /// names its arguments as the class header does and stands at the
    /// header's factory pattern (decision 12).
    fn a_factory_frame_names_its_parameters(lowering) {
        let (frames, variables) = while_stopped_under(lowering, FACTORIES, |remote| {
            frame_and_variables(remote, "Made class>>x:y:")
        });
        let factory = frame_named(&frames, "Made class>>x:y:");
        assert_eq!(frames[factory - 1].name, "Made initializer");
        assert_eq!(frames[factory + 1].name, "Factories>>run");
        let location = frames[factory]
            .location
            .clone()
            .expect("the factory is located");
        assert_eq!(location.module, "Factories");
        assert_eq!(location.statement.text(FACTORIES), "x: first y: second");
        assert_eq!(names(&variables), ["self", "first", "second"]);
        assert_eq!(value_of(&variables, "first").label, "3");
        assert_eq!(value_of(&variables, "second").label, "4");
    }
}

/// A method that replaces itself through a MirrorBuilder and then loops
/// in a block of its superseded body, which the module's text no longer
/// holds.
const CHANGING: &str = indoc! {"
    class Changing usingPlatform: platform = Object new (
      | mirrors = platform mirrors. |
    ) (
      public class Subject = Object new () (
        public spin: count = (
          | items block |
          items:: Array new: 2.
          change.
          block:: [:limit | [true] whileTrue: []. limit].
          ^block value: count
        )
      )
      public run = (
        ^Subject new spin: 5
      )
      public change = (
        | builder = (mirrors ClassMirror reflecting: Subject) mixin declaration asBuilder. |
        builder instanceSide methods addFromSource: 'public spin: count = ( ^count )'.
        builder install
      )
    ) : ()
"};

/// The index of the frame named `[] in Subject>>spin:` nearest the
/// method's: the block `spin:` creates, below the loop's blocks the
/// reference lowering sends `value` to.
fn spinning_block(frames: &[FrameDescription]) -> usize {
    frames
        .iter()
        .rposition(|frame| frame.name == "[] in Subject>>spin:")
        .unwrap_or_else(|| {
            panic!(
                "a frame of spin:'s block was expected in {:?}",
                frame_names(frames)
            )
        })
}

/// The stack and the variables of the frame named `Subject>>spin:` and of
/// its block's (`spinning_block`) of `source`'s `run` under `lowering`,
/// sent to the module's instance made `usingPlatform:`.
fn spinning_subject(
    source: &'static str,
    lowering: Lowering,
) -> (Vec<FrameDescription>, Vec<Variable>, Vec<Variable>) {
    let (mut interpreter, class) = load(source, lowering);
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
        "usingPlatform:",
    );
    let ((frames, method, block), _, _) =
        while_stopped_in(interpreter, &instance, PAUSE_DELAY, |remote| {
            let (frames, method) = frame_and_variables(remote, "Subject>>spin:");
            let block = variables(
                remote,
                Command::Variables {
                    frame: spinning_block(&frames),
                },
            );
            (frames, method, block)
        });
    (frames, method, block)
}

ns_runtime::for_each_lowering! {
    /// A frame running a body its module's text no longer compiles to has
    /// no metadata: it keeps its name, has no location and shows its
    /// arguments and registers by number (decision 5). Its version is
    /// found all the same: its block's frame is named for the method that
    /// creates it, and has no metadata either, since the family's root
    /// does not compile alike.
    fn a_changed_superseded_body_shows_unnamed_registers(lowering) {
        let (frames, method, block) = spinning_subject(CHANGING, lowering);
        let spinning = frame_named(&frames, "Subject>>spin:");
        assert_eq!(frames[spinning].location, None);
        assert_eq!(method[0].name, "self");
        assert_eq!(value_of(&method, "argument 1").label, "5");
        let items = method
            .iter()
            .find(|variable| shown(variable).class_name == "Array")
            .expect("a register holds the Array");
        assert!(items.name.starts_with("register "), "{}", items.name);
        let looping = spinning_block(&frames);
        assert_eq!(frames[looping].location, None);
        assert_eq!(names(&block), ["self", "argument 1"]);
        assert_eq!(value_of(&block, "argument 1").label, "5");
    }
}

/// A method whose running adds another method to its class through a
/// MirrorBuilder, and then loops in a block of its own body, which the
/// install superseded but did not change.
const ADDING: &str = indoc! {"
    class Adding usingPlatform: platform = Object new (
      | mirrors = platform mirrors. |
    ) (
      public class Subject = Object new () (
        public spin: count = (
          | items block |
          items:: Array new: 2.
          change.
          block:: [:limit | [true] whileTrue: []. limit].
          ^block value: count
        )
      )
      public run = (
        ^Subject new spin: 5
      )
      public change = (
        | builder = (mirrors ClassMirror reflecting: Subject) mixin declaration asBuilder. |
        builder instanceSide methods addFromSource: 'public other = ( ^7 )'.
        builder install
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// An install rebuilds every body of the declaration it edits, so a
    /// frame still running one runs a superseded version. When the edit
    /// left its body alone, the version compiles alike with the current
    /// text, and the frame and its block's keep their names, their
    /// location and their named variables (decision 11).
    fn an_unchanged_superseded_body_keeps_its_metadata(lowering) {
        let (frames, method, block) = spinning_subject(ADDING, lowering);
        let spinning = frame_named(&frames, "Subject>>spin:");
        let location = frames[spinning]
            .location
            .clone()
            .expect("the superseded method is located");
        assert_eq!(location.module, "Adding");
        assert_eq!(location.statement.text(ADDING), "^block value: count");
        assert_eq!(names(&method), ["self", "count", "items", "block"]);
        assert_eq!(value_of(&method, "count").label, "5");
        assert_eq!(value_of(&method, "items").class_name, "Array");
        let looping = spinning_block(&frames);
        let location = frames[looping]
            .location
            .clone()
            .expect("the superseded block is located");
        assert!(
            ["[true] whileTrue: []", "true"].contains(&location.statement.text(ADDING)),
            "{}",
            location.statement.text(ADDING)
        );
        assert_eq!(value_of(&block, "limit").label, "5");
        assert_eq!(value_of(&block, "count").label, "5");
    }
}

/// A frame of a module an image carries without its source has no
/// metadata: no location, and its arguments by number.
#[test]
fn an_image_without_source_shows_unnamed_frames() {
    let (mut interpreter, class) = load(STACKED, Lowering::Optimized);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let origin = ns_runtime::SourceOrigin::File("test.ns".into());
    let bytes = interpreter
        .enter(|mutator| {
            let class = mutator.fetch(&class).as_object().expect("the module class");
            let modules = [ns_runtime::image::ModuleSource {
                class,
                origin: &origin,
                with_source: false,
            }];
            ns_runtime::image::snapshot(mutator, &modules, &[mutator.fetch(&instance)])
        })
        .unwrap_or_else(|error| panic!("{error}"));
    let mut image = ns_runtime::image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
    let instance = image.roots.remove(0);
    let ((frames, variables), _, _) =
        while_stopped_in(image.interpreter, &instance, PAUSE_DELAY, |remote| {
            frame_and_variables(remote, "Stacked>>loop:with:")
        });
    let looping = frame_named(&frames, "Stacked>>loop:with:");
    assert_eq!(frames[looping].location, None);
    assert_eq!(names(&variables), ["self", "argument 1", "argument 2"]);
    assert_eq!(value_of(&variables, "argument 1").label, "5");
}

/// A block stored by a method that returned, as `RETURNED`'s, with the
/// method sent before the image is written and the block evaluated
/// after it is loaded.
const KEPT: &str = indoc! {"
    class Kept = Object new (
      | block |
    ) (
      public prepare = (
        self make: 3
      )
      make: limit = (
        | unused |
        unused:: 42.
        block:: [
          | count |
          count:: limit.
          [true] whileTrue: [count:: count + 1]
        ]
      )
      public run = (
        ^block value
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A returned creator's frame an image decoded is read through the
    /// closure that reads it by reference -- the reference lowering's
    /// block -- since its record does not say whether it kept its
    /// registers; the optimized lowering's block reads none of it, and
    /// the variable it did not copy is not available (decision 10).
    fn a_decoded_creator_s_variable_is_read_through_a_closure_reading_it(lowering) {
        let (mut interpreter, class) = load(KEPT, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        expect_ok(
            interpreter.send(&instance, Symbol::intern("prepare"), &[]),
            "prepare",
        );
        let origin = ns_runtime::SourceOrigin::File("test.ns".into());
        let bytes = interpreter
            .enter(|mutator| {
                let class = mutator.fetch(&class).as_object().expect("the module class");
                let modules = [ns_runtime::image::ModuleSource {
                    class,
                    origin: &origin,
                    with_source: true,
                }];
                ns_runtime::image::snapshot(mutator, &modules, &[mutator.fetch(&instance)])
            })
            .unwrap_or_else(|error| panic!("{error}"));
        let mut image = ns_runtime::image::load(&bytes).unwrap_or_else(|error| panic!("{error}"));
        let instance = image.roots.remove(0);
        let ((_, variables), _, _) =
            while_stopped_in(image.interpreter, &instance, PAUSE_DELAY, |remote| {
                frame_and_variables(remote, "[] in Kept>>make:")
            });
        assert_eq!(value_of(&variables, "limit").label, "3");
        let unused = named(&variables, "unused");
        match lowering {
            Lowering::Optimized => assert_eq!(unused.value, VariableValue::Unavailable),
            Lowering::Reference => assert_eq!(shown(unused).label, "42"),
        }
    }
}

/// A method temporary its block assigns, which the optimized lowering
/// keeps in an indirection vector, read at a stop in a block nested in
/// that block, which never names it.
const VECTORED: &str = indoc! {"
    class Vectored = Object new () (
      public run = (
        ^self outer: 5
      )
      outer: count = (
        | total |
        total:: count.
        ^[:step |
          total:: total + step.
          [[true] whileTrue: []] value
        ] value: 2
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A variable in an indirection vector shows its current value in
    /// every frame of its chain: the method's, which holds the vector; the
    /// block that assigns it, which was handed the vector; and the block
    /// nested in it, which reads it through the vector its creator holds
    /// (decision 10). The reference lowering reads the same variable by
    /// reference, and shows the same names and values.
    fn a_vector_variable_shows_its_current_value(lowering) {
        let (method, blocks) = while_stopped_under(lowering, VECTORED, |remote| {
            let frames = stack_trace(remote);
            let blocks: Vec<Vec<Variable>> = frames
                .iter()
                .enumerate()
                .filter(|(_, frame)| frame.name == "[] in Vectored>>outer:")
                .map(|(frame, _)| variables(remote, Command::Variables { frame }))
                .collect();
            let (_, method) = frame_and_variables(remote, "Vectored>>outer:");
            (method, blocks)
        });
        assert_eq!(names(&method), ["self", "count", "total"]);
        assert_eq!(value_of(&method, "total").label, "7");
        assert!(
            blocks.len() >= 2,
            "the assigning block and the one nested in it, not {}",
            blocks.len()
        );
        for variables in &blocks {
            assert_eq!(value_of(variables, "step").label, "2");
            assert_eq!(value_of(variables, "count").label, "5");
            assert_eq!(value_of(variables, "total").label, "7");
        }
    }
}

ns_runtime::for_each_lowering! {
    /// Every body of every shipped module, the lazy ones built first,
    /// compiles again alike under the interpreter's lowering, and its
    /// tables hold their invariants (decision 14): the shipped modules
    /// are checked here once, not with every golden.
    fn every_shipped_body_compiles_again_alike(lowering) {
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        match interpreter.check_debug_metadata(true) {
            Ok(count) => assert!(count > 0, "no shipped body was checked"),
            Err(failure) => panic!("{failure}"),
        }
    }
}

/// `source`, parsed and resolved, for a module loaded after the first.
fn resolved_module(source: &str) -> ns_runtime::types::ResolvedModule {
    let module = ns_syntax::parse(source).expect("parse");
    ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("later.ns".into()),
    )
    .expect("resolve")
}

/// An interpreter compiling with `lowering`, its JIT off, so that every
/// breakpoint is the interpreter's, with `source` loaded, and its module
/// class.
fn interpreted(source: &str, lowering: Lowering) -> (Interpreter, Root) {
    let (mut interpreter, class) = load(source, lowering);
    interpreter.set_jit_enabled(false);
    (interpreter, class)
}

/// Runs `source`'s `run` under `lowering`, the JIT off, with a debugger
/// attached: `configure` queues its commands on the test thread before
/// the run, which serves them as it starts, and `agent` serves the run
/// from another thread. Answers what `agent` answered and the run's
/// result.
fn debugged<T: Send + 'static>(
    source: &str,
    lowering: Lowering,
    configure: impl FnOnce(&DebugRemote),
    agent: impl FnOnce(&DebugRemote) -> T + Send + 'static,
) -> (T, HostResult) {
    let (mut interpreter, class) = interpreted(source, lowering);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    debugged_in(&mut interpreter, &instance, configure, agent)
}

/// `debugged` for `instance` of `interpreter`.
fn debugged_in<T: Send + 'static>(
    interpreter: &mut Interpreter,
    instance: &Root,
    configure: impl FnOnce(&DebugRemote),
    agent: impl FnOnce(&DebugRemote) -> T + Send + 'static,
) -> (T, HostResult) {
    let remote = interpreter.attach_debugger();
    configure(&remote);
    let (answer, result, _) = debugged_by(remote, agent, || run(interpreter, instance));
    (answer, result)
}

thread_local! {
    /// On the agent's thread of `debugged_by`, set once the run it serves
    /// ended: no event says so, and an agent that waits for the silence of
    /// a while instead could take a slow run's pause for its end.
    static RUN_FINISHED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Whether the run the agent's thread serves has ended; only an agent of
/// `debugged_by` can ask.
fn run_finished() -> bool {
    RUN_FINISHED.with(|flag| {
        flag.borrow()
            .as_ref()
            .expect("only an agent of `debugged_by` waits for its run's end")
            .load(Ordering::SeqCst)
    })
}

/// Runs what `run` runs while `agent` serves `remote` from another
/// thread, and answers what `agent` answered, the run's result and the
/// remote. The agent's thread goes on continuing every stop once `agent`
/// returned, until the run ended, and terminates the run if `agent`
/// panicked, so that a failing assertion fails the test rather than
/// leaving the run stopped.
fn debugged_by<T: Send + 'static>(
    remote: DebugRemote,
    agent: impl FnOnce(&DebugRemote) -> T + Send + 'static,
    run: impl FnOnce() -> HostResult,
) -> (T, HostResult, DebugRemote) {
    let finished = Arc::new(AtomicBool::new(false));
    let thread = {
        let finished = Arc::clone(&finished);
        thread::spawn(move || {
            RUN_FINISHED.with(|flag| *flag.borrow_mut() = Some(Arc::clone(&finished)));
            let answer = panic::catch_unwind(AssertUnwindSafe(|| agent(&remote)));
            if answer.is_err() {
                remote.send(Command::Terminate);
            }
            while !finished.load(Ordering::SeqCst) {
                if let Some(Event::Stopped { .. }) = remote.next_event(Duration::from_millis(10)) {
                    remote.send(Command::Continue);
                }
            }
            // Held until the run ended, so that the run does not take its
            // departure for a detach.
            (answer, remote)
        })
    };
    let result = run();
    finished.store(true, Ordering::SeqCst);
    let (answer, remote) = thread.join().expect("the agent's thread");
    let answer = answer.unwrap_or_else(|failure| panic::resume_unwind(failure));
    (answer, result, remote)
}

/// The offset of the start of the line of `source` that holds `text`.
fn line_start(source: &str, text: &str) -> u32 {
    let found = source
        .find(text)
        .unwrap_or_else(|| panic!("the source holds {text}"));
    let start = source[..found].rfind('\n').map_or(0, |newline| newline + 1);
    u32::try_from(start).expect("a test's source is small")
}

/// Sets the line breakpoints of `module` at `offsets`, each stopping at
/// every hit.
fn set_breakpoints(remote: &DebugRemote, module: &str, offsets: Vec<u32>) {
    let breakpoints = offsets.into_iter().map(LineBreakpoint::at).collect();
    set_line_breakpoints(remote, module, breakpoints);
}

/// Sets the line breakpoints of `module` to `breakpoints`.
fn set_line_breakpoints(remote: &DebugRemote, module: &str, breakpoints: Vec<LineBreakpoint>) {
    remote.send(Command::SetBreakpoints {
        module: String::from(module),
        breakpoints,
    });
}

/// Waits for the breakpoints of `module`: the answer to `SetBreakpoints`,
/// or the report of a resolution.
fn expect_breakpoints(remote: &DebugRemote, module: &str) -> Vec<BreakpointState> {
    match remote.next_event(EVENT_TIMEOUT) {
        Some(Event::Breakpoints {
            module: reported,
            breakpoints,
        }) if reported == module => breakpoints,
        other => panic!("the breakpoints of {module} were expected, not {other:?}"),
    }
}

/// The one breakpoint of `breakpoints`, which resolved to a statement:
/// its id and the statement.
fn resolved(breakpoints: &[BreakpointState]) -> (BreakpointId, ns_syntax::Span) {
    match breakpoints {
        [only] => (
            only.id,
            only.statement
                .unwrap_or_else(|| panic!("the breakpoint at {} resolves", only.offset)),
        ),
        other => panic!("one breakpoint was expected, not {other:?}"),
    }
}

/// The one breakpoint of `breakpoints`, which is pending: its id.
fn pending(breakpoints: &[BreakpointState]) -> BreakpointId {
    match breakpoints {
        [only] if only.statement.is_none() => only.id,
        other => panic!("one pending breakpoint was expected, not {other:?}"),
    }
}

/// Waits for a stop at a breakpoint and answers the ids it names.
fn expect_breakpoint_stop(remote: &DebugRemote) -> Vec<BreakpointId> {
    match remote.next_event(EVENT_TIMEOUT) {
        Some(Event::Stopped {
            reason: StopReason::Breakpoint { ids },
            returned: None,
        }) => ids,
        other => panic!("a stop at a breakpoint was expected, not {other:?}"),
    }
}

/// The small integer a run answered.
fn answered(interpreter: &mut Interpreter, result: HostResult) -> i64 {
    let root = expect_ok(result, "run");
    small_integer(interpreter, &root)
}

/// A method of three statements, each of which changes the slot.
const THREE_STATEMENTS: &str = indoc! {"
    class Lines = Object new (
      | count ::= 0. |
    ) (
      public run = (
        count:: count + 1.
        count:: count + 10.
        ^count + 100
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A breakpoint at a line's start resolves to the statement that
    /// starts there, and stops the run with its frame standing at it;
    /// continued, the run goes on to its unchanged outcome.
    fn a_line_breakpoint_stops_at_its_statement(lowering) {
        let offset = line_start(THREE_STATEMENTS, "count:: count + 10.");
        let ((statement, location), result) = debugged(
            THREE_STATEMENTS,
            lowering,
            |remote| set_breakpoints(remote, "Lines", vec![offset]),
            |remote| {
                let (id, statement) = resolved(&expect_breakpoints(remote, "Lines"));
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                let location = stack_trace(remote)[0].location.clone();
                continue_stopped(remote);
                (statement, location)
            },
        );
        assert_eq!(statement.text(THREE_STATEMENTS), "count:: count + 10");
        let location = location.expect("the stopped frame is located");
        assert_eq!(location.module, "Lines");
        assert_eq!(location.statement, statement);
        expect_ok(result, "run");
    }
}

ns_runtime::for_each_lowering! {
    /// A `SetBreakpoints` queued as the run goes on from one breakpoint,
    /// and served at the safepoint the next one, which no poll comes
    /// before, left for, removes that one before its stop is made: the
    /// stop is decided afresh from the table, and no stop names a
    /// breakpoint after the answer that removed it. The command is sent
    /// right after the `Continue`, long before the run wakes and reaches
    /// the second statement; should it ever arrive later, the stop it
    /// would make comes before the answer, and the test still holds.
    fn a_breakpoint_removed_before_its_stop_is_served_is_not_reported(lowering) {
        let first = line_start(THREE_STATEMENTS, "count:: count + 1.");
        let second = line_start(THREE_STATEMENTS, "count:: count + 10.");
        let ((removed, events), answer) = stepped(
            THREE_STATEMENTS,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Lines", vec![first, second]),
            move |remote| {
                let breakpoints = expect_breakpoints(remote, "Lines");
                let ids: Vec<BreakpointId> = breakpoints.iter().map(|state| state.id).collect();
                assert_eq!(expect_breakpoint_stop(remote), [ids[0]]);
                remote.send(Command::Continue);
                set_breakpoints(remote, "Lines", vec![first]);
                (ids[1], events_until_the_end(remote))
            },
        );
        let answered = events
            .iter()
            .position(|event| matches!(event, Event::Breakpoints { module, .. } if module == "Lines"))
            .expect("the removal is answered");
        let stale: Vec<&Event> = events[answered..]
            .iter()
            .filter(|event| {
                matches!(event, Event::Stopped { reason: StopReason::Breakpoint { ids }, .. }
                    if ids.contains(&removed))
            })
            .collect();
        assert!(stale.is_empty(), "a stop names a removed breakpoint: {stale:?}");
        // The third run of the instance, after the two that warmed it.
        assert_eq!(answer, 133);
    }
}

ns_runtime::for_each_lowering! {
    /// The run a breakpoint stopped answers what it answers undebugged.
    fn a_stopped_run_answers_its_unchanged_outcome(lowering) {
        let (mut interpreter, class) = interpreted(THREE_STATEMENTS, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let offset = line_start(THREE_STATEMENTS, "count:: count + 10.");
        let ((), result) = debugged_in(
            &mut interpreter,
            &instance,
            |remote| set_breakpoints(remote, "Lines", vec![offset]),
            |remote| {
                let (id, _) = resolved(&expect_breakpoints(remote, "Lines"));
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                continue_stopped(remote);
            },
        );
        assert_eq!(answered(&mut interpreter, result), 111);
    }
}

/// A loop whose body counts its passes.
const LOOPING: &str = indoc! {"
    class Looping = Object new (
      | count ::= 0. passes ::= 0. |
    ) (
      public run = (
        1 to: 3 do: [:index |
          passes:: passes + 1.
          count:: count + index].
        ^count * 10 + passes
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A breakpoint in a loop's body stops at every pass, and each
    /// statement of the body runs once per pass.
    fn a_breakpoint_in_a_loop_stops_every_iteration(lowering) {
        let (mut interpreter, class) = interpreted(LOOPING, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let offset = line_start(LOOPING, "passes:: passes + 1.");
        let (statement, result) = debugged_in(
            &mut interpreter,
            &instance,
            |remote| set_breakpoints(remote, "Looping", vec![offset]),
            |remote| {
                let (id, statement) = resolved(&expect_breakpoints(remote, "Looping"));
                for _ in 0..3 {
                    assert_eq!(expect_breakpoint_stop(remote), [id]);
                    continue_stopped(remote);
                }
                statement
            },
        );
        assert_eq!(statement.text(LOOPING), "passes:: passes + 1");
        assert_eq!(answered(&mut interpreter, result), 63);
    }
}

/// A send that increments a slot, as a statement of its own.
const BUMPING: &str = indoc! {"
    class Bumping = Object new (
      | count ::= 0. |
    ) (
      public run = (
        self bump.
        ^count
      )
      public bump = (
        count:: count + 1
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// Removing the breakpoint the run stopped at gives the slot its
    /// original back, which the continued run executes once.
    fn removing_the_breakpoint_one_stands_at_runs_the_original_once(lowering) {
        let (mut interpreter, class) = interpreted(BUMPING, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let offset = line_start(BUMPING, "self bump.");
        let (left, result) = debugged_in(
            &mut interpreter,
            &instance,
            |remote| set_breakpoints(remote, "Bumping", vec![offset]),
            |remote| {
                let (id, _) = resolved(&expect_breakpoints(remote, "Bumping"));
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                set_breakpoints(remote, "Bumping", Vec::new());
                let left = expect_breakpoints(remote, "Bumping");
                continue_stopped(remote);
                left
            },
        );
        assert_eq!(left, []);
        assert_eq!(answered(&mut interpreter, result), 1);
    }
}

#[cfg(debug_assertions)]
ns_runtime::for_each_lowering! {
    /// The breakpoint stress mode's patch and a line breakpoint stand at
    /// the same slot, whichever came first: the line breakpoint stops at
    /// every pass, the stress patch never does, and once the line
    /// breakpoint is removed the slot, still the stress mode's, stops no
    /// more.
    fn a_stress_patch_beside_a_line_breakpoint_never_stops(lowering) {
        let (mut interpreter, class) = interpreted(LOOPING, lowering);
        interpreter.set_stress_breakpoints(true);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        // The stress mode patches the loop's statements first.
        let result = run(&mut interpreter, &instance);
        assert_eq!(answered(&mut interpreter, result), 63);
        let before = interpreter.stress_breakpoint_hits();
        assert!(before > 0, "the stress mode patched the loop");
        let offset = line_start(LOOPING, "passes:: passes + 1.");
        let (stops, result) = debugged_in(
            &mut interpreter,
            &instance,
            |remote| set_breakpoints(remote, "Looping", vec![offset]),
            |remote| {
                let (id, _) = resolved(&expect_breakpoints(remote, "Looping"));
                for _ in 0..3 {
                    assert_eq!(expect_breakpoint_stop(remote), [id]);
                    continue_stopped(remote);
                }
                3
            },
        );
        assert_eq!(stops, 3);
        assert_eq!(answered(&mut interpreter, result), 126);
        let remote = interpreter.attach_debugger();
        set_breakpoints(&remote, "Looping", Vec::new());
        let (events, result, _remote) = events_during(remote, || run(&mut interpreter, &instance));
        assert_no_stop(&events);
        assert!(reports_no_breakpoint(&events, "Looping"));
        assert_eq!(answered(&mut interpreter, result), 189);
        assert!(interpreter.stress_breakpoint_hits() > before);
    }
}

/// An `ifTrue:` sent once to a Boolean, whose block the compiler inlines,
/// and once to an object that answers it by running its argument, the
/// block's fallback closure.
const FALLING_BACK: &str = indoc! {"
    class FallingBack = Object new (
      | count ::= 0. |
    ) (
      public run = (
        self check: true.
        self check: Yes new.
        ^count
      )
      public check: condition = (
        condition ifTrue: [
          count:: count + 1].
        ^count
      )
      public class Yes = Object new () (
        public ifTrue: block = (
          ^block value
        )
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A breakpoint in an inlined block patches both of its copies: the
    /// one inline in its home and the fallback closure a receiver that is
    /// no Boolean runs.
    fn a_breakpoint_in_a_block_and_in_its_fallback_both_stop(lowering) {
        let (mut interpreter, class) = interpreted(FALLING_BACK, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let offset = line_start(FALLING_BACK, "count:: count + 1].");
        let (statement, result) = debugged_in(
            &mut interpreter,
            &instance,
            |remote| set_breakpoints(remote, "FallingBack", vec![offset]),
            |remote| {
                let (id, statement) = resolved(&expect_breakpoints(remote, "FallingBack"));
                for _ in 0..2 {
                    assert_eq!(expect_breakpoint_stop(remote), [id]);
                    continue_stopped(remote);
                }
                statement
            },
        );
        assert_eq!(statement.text(FALLING_BACK), "count:: count + 1");
        assert_eq!(answered(&mut interpreter, result), 2);
    }
}

/// A loop that never ends, its body a statement of its own line.
const SPINNING: &str = indoc! {"
    class Spinning = Object new (
      | count ::= 0. |
    ) (
      public run = (
        [true] whileTrue: [
          count:: count + 1].
        ^count
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A breakpoint set while the program runs is served at the next
    /// safepoint without stopping, and the loop then stops at it.
    fn a_breakpoint_set_while_running_takes_effect(lowering) {
        let (mut interpreter, class) = interpreted(SPINNING, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let offset = line_start(SPINNING, "count:: count + 1].");
        let (statement, result) = debugged_in(
            &mut interpreter,
            &instance,
            |_| {},
            move |remote| {
                thread::sleep(PAUSE_DELAY);
                set_breakpoints(remote, "Spinning", vec![offset]);
                let (id, statement) = resolved(&expect_breakpoints(remote, "Spinning"));
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                remote.send(Command::Terminate);
                statement
            },
        );
        assert_eq!(statement.text(SPINNING), "count:: count + 1");
        assert_terminated(result);
    }
}

/// A module loaded after the program's first run.
const LATER: &str = indoc! {"
    class Later = Object new () (
      public answer = (
        | value |
        value:: 6 * 7.
        ^value
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A breakpoint in a module not loaded yet is pending; loading the
    /// module resolves it, reported unasked, and its method then stops.
    fn a_pending_breakpoint_resolves_when_its_module_loads(lowering) {
        let (mut interpreter, class) = interpreted(THREE_STATEMENTS, lowering);
        let remote = interpreter.attach_debugger();
        set_breakpoints(&remote, "Later", vec![line_start(LATER, "value:: 6 * 7.")]);
        // The instantiation is a run, which serves the command as it
        // starts.
        expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let id = pending(&expect_breakpoints(&remote, "Later"));
        let later_class = interpreter.load_module(&resolved_module(LATER));
        let (resolved_id, statement) = resolved(&expect_breakpoints(&remote, "Later"));
        assert_eq!(resolved_id, id);
        assert_eq!(statement.text(LATER), "value:: 6 * 7");
        let later = expect_ok(interpreter.instantiate(&later_class, &[]), "instantiate Later");
        let ((), result, _) = debugged_by(
            remote,
            move |remote| {
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                continue_stopped(remote);
            },
            || interpreter.send(&later, Symbol::intern("answer"), &[]),
        );
        assert_eq!(answered(&mut interpreter, result), 42);
    }
}

/// The text of the shipped module `TranscriptFactory`, which its spans
/// count in.
const TRANSCRIPT_FACTORY: &str = include_str!("../core-library/platform/TranscriptFactory.ns");

/// A program whose run asks the platform for its transcript, which builds
/// the shipped module `TranscriptFactory` on first demand.
const PRINTING: &str = indoc! {"
    class Printing usingPlatform: platform = Object new (
      | thePlatform = platform. |
    ) (
      public run = (
        ^thePlatform transcript
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A breakpoint in a shipped module built on first demand is pending
    /// until the program first asks for the module, and stops at its
    /// first use.
    fn a_breakpoint_in_a_lazily_built_shipped_module_resolves_when_it_is_built(lowering) {
        let (mut interpreter, class) = interpreted(PRINTING, lowering);
        let platform = interpreter.platform();
        let instance = expect_ok(
            interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
            "usingPlatform:",
        );
        let offset = line_start(TRANSCRIPT_FACTORY, "^Transcript new.");
        let ((location, statement), result) = debugged_in(
            &mut interpreter,
            &instance,
            |remote| set_breakpoints(remote, "TranscriptFactory", vec![offset]),
            |remote| {
                let id = pending(&expect_breakpoints(remote, "TranscriptFactory"));
                let (resolved_id, statement) =
                    resolved(&expect_breakpoints(remote, "TranscriptFactory"));
                assert_eq!(resolved_id, id);
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                let location = stack_trace(remote)[0].location.clone();
                continue_stopped(remote);
                (location, statement)
            },
        );
        assert_eq!(statement.text(TRANSCRIPT_FACTORY), "^Transcript new");
        let location = location.expect("the stopped frame is located");
        assert_eq!(location.module, "TranscriptFactory");
        assert_eq!(location.statement, statement);
        expect_ok(result, "run");
    }
}

/// A method whose running replaces another method through a
/// MirrorBuilder before it sends it; the replacement declares one more
/// temporary, so that its statement starts later in the module's text.
const EDITING: &str = indoc! {"
    class Editing usingPlatform: platform = Object new (
      | mirrors = platform mirrors. |
    ) (
      public class Subject = Object new () (
        public answer = ( | value | value:: 6 * 7. ^value )
      )
      public run = (
        change.
        ^Subject new answer
      )
      public change = (
        | builder = (mirrors ClassMirror reflecting: Subject) mixin declaration asBuilder. |
        builder instanceSide methods addFromSource: 'public answer = ( | value other | value:: 6 * 8. ^value )'.
        builder install
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// An install replaces the bodies of the class it edits: a breakpoint
    /// in one is set again on the new body by its offset, reported
    /// unasked with its new statement, and the new body stops.
    fn an_install_sets_the_breakpoint_again_on_the_new_body(lowering) {
        let (mut interpreter, class) = interpreted(EDITING, lowering);
        let platform = interpreter.platform();
        let instance = expect_ok(
            interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
            "usingPlatform:",
        );
        let offset = EDITING.find("value:: 6 * 7").expect("the first answer's statement");
        let offset = u32::try_from(offset).expect("a test's source is small");
        let ((first, moved, stopped_at, name), result) = debugged_in(
            &mut interpreter,
            &instance,
            |remote| set_breakpoints(remote, "Editing", vec![offset]),
            |remote| {
                let (id, first) = resolved(&expect_breakpoints(remote, "Editing"));
                let (moved_id, moved) = resolved(&expect_breakpoints(remote, "Editing"));
                assert_eq!(moved_id, id);
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                let frames = stack_trace(remote);
                let stopped_at = frames[0]
                    .location
                    .clone()
                    .map(|location| location.statement);
                continue_stopped(remote);
                (first, moved, stopped_at, frames[0].name.clone())
            },
        );
        assert_eq!(first.text(EDITING), "value:: 6 * 7");
        assert_ne!(moved, first);
        assert_eq!(stopped_at, Some(moved));
        assert_eq!(name, "Subject>>answer");
        assert_eq!(answered(&mut interpreter, result), 48);
    }
}

#[cfg(debug_assertions)]
ns_runtime::for_each_lowering! {
    /// A full collection keeps a patched body's code id, though no
    /// running activation names it; the breakpoint removed after it gives
    /// the body its original back, which the next run executes.
    fn a_patched_body_keeps_its_code_id_across_a_full_collection(lowering) {
        let (mut interpreter, class) = interpreted(BUMPING, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let remote = interpreter.attach_debugger();
        let offset = line_start(BUMPING, "count:: count + 1");
        set_breakpoints(&remote, "Bumping", vec![offset]);
        let ((), result, remote) = debugged_by(
            remote,
            |remote| {
                let (id, _) = resolved(&expect_breakpoints(remote, "Bumping"));
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                continue_stopped(remote);
            },
            || run(&mut interpreter, &instance),
        );
        assert_eq!(answered(&mut interpreter, result), 1);
        let bump = Symbol::intern("bump");
        let before = interpreter.jit_code_id(&class, MethodSide::Instance, bump);
        interpreter.collect_now();
        let after = interpreter.jit_code_id(&class, MethodSide::Instance, bump);
        assert_eq!(after, before, "the patched body keeps its code id");
        set_breakpoints(&remote, "Bumping", Vec::new());
        let (events, result, _) = events_during(remote, || run(&mut interpreter, &instance));
        assert_no_stop(&events);
        assert!(reports_no_breakpoint(&events, "Bumping"), "{events:?}");
        assert_eq!(answered(&mut interpreter, result), 2);
    }
}

/// Runs what `run` runs while another thread collects every event
/// `remote` reports, and answers them, the run's result and the remote.
/// A stop is continued, so that a test that expects none fails at its
/// assertion rather than leaving the run stopped.
fn events_during(
    remote: DebugRemote,
    run: impl FnOnce() -> HostResult,
) -> (Vec<Event>, HostResult, DebugRemote) {
    let finished = Arc::new(AtomicBool::new(false));
    let thread = {
        let finished = Arc::clone(&finished);
        thread::spawn(move || {
            let mut events = Vec::new();
            loop {
                let done = finished.load(Ordering::SeqCst);
                match remote.next_event(Duration::from_millis(10)) {
                    Some(event) => {
                        if let Event::Stopped { .. } = event {
                            remote.send(Command::Continue);
                        }
                        events.push(event);
                    }
                    // Read once more after the run ended, so that nothing
                    // it sent last is missed.
                    None if done => break,
                    None => {}
                }
            }
            (events, remote)
        })
    };
    let result = run();
    finished.store(true, Ordering::SeqCst);
    let (events, remote) = thread.join().expect("the collecting thread");
    (events, result, remote)
}

/// Asserts that no event of `events` is a stop.
fn assert_no_stop(events: &[Event]) {
    let stops: Vec<&Event> = events
        .iter()
        .filter(|event| matches!(event, Event::Stopped { .. }))
        .collect();
    assert!(
        stops.is_empty(),
        "the run was expected not to stop: {stops:?}"
    );
}

/// Whether `events` hold the answer that `module` has no breakpoint.
fn reports_no_breakpoint(events: &[Event], module: &str) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            Event::Breakpoints { module: reported, breakpoints }
                if reported == module && breakpoints.is_empty()
        )
    })
}

/// The small integer the unary `selector` sent to `instance` answers.
fn send_answered(interpreter: &mut Interpreter, instance: &Root, selector: &str) -> i64 {
    let result = interpreter.send(instance, Symbol::intern(selector), &[]);
    answered(interpreter, result)
}

/// `loop:` with `turns` sent to `instance`.
fn send_loop(interpreter: &mut Interpreter, instance: &Root, turns: i64) -> HostResult {
    let turns = interpreter.stash(ns_runtime::Value::small_integer(turns));
    interpreter.send(instance, Symbol::intern("loop:"), &[turns])
}

/// The small integer `loop:` with `turns` sent to `instance` answers.
fn loop_answered(interpreter: &mut Interpreter, instance: &Root, turns: i64) -> i64 {
    let result = send_loop(interpreter, instance, turns);
    answered(interpreter, result)
}

/// Whether these tests' bodies run in machine code under `lowering`, so
/// that the JIT's counters say what they assert: the reference lowering
/// sends uncached, and machine code leaves every send of it to the
/// interpreter, so that a body that sends is never installed.
fn runs_in_machine_code(lowering: Lowering) -> bool {
    lowering == Lowering::Optimized && counters_are_exact()
}

/// An interpreter compiling with `lowering`, its JIT on, with `source`
/// loaded, an instance of its module class, and the class: every body
/// stays in baseline machine code, its optimization budget parked, and a
/// loop is compiled at its first interpreted backward jump.
fn compiled(source: &str, lowering: Lowering) -> (Interpreter, Root, Root) {
    let (mut interpreter, class) = load(source, lowering);
    interpreter.set_optimization_budget(i64::MAX);
    interpreter.set_jit_compile_at_back_jump(1);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    (interpreter, instance, class)
}

/// A loop whose every pass notes its index, a compiled send, and bumps
/// the count, a statement of its own: `warm` runs it 2,000 times, `run`
/// three times, `long` a hundred times.
const COMPILED_LOOP: &str = indoc! {"
    class CompiledLoop = Object new (
      | count ::= 0. |
    ) (
      public warm = (
        ^self loop: 2000
      )
      public run = (
        ^self loop: 3
      )
      public long = (
        ^self loop: 100
      )
      loop: turns = (
        1 to: turns do: [:index |
          self note: index.
          self bump].
        ^count
      )
      note: index = (
        ^index
      )
      bump = (
        count:: count + 1
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A breakpoint in a loop that runs in machine code frees the code
    /// and stops at every pass, and each pass sends `bump` once: the body
    /// compiled again exits at the patched statement, which the
    /// interpreter stops at and then executes.
    fn a_breakpoint_in_compiled_loop_stops_every_iteration_and_sends_once(lowering) {
        let (mut interpreter, instance, _) = compiled(COMPILED_LOOP, lowering);
        assert_eq!(send_answered(&mut interpreter, &instance, "warm"), 2000);
        let before = interpreter.jit_statistics();
        let offset = line_start(COMPILED_LOOP, "self bump].");
        let ((), result) = debugged_in(
            &mut interpreter,
            &instance,
            |remote| set_breakpoints(remote, "CompiledLoop", vec![offset]),
            |remote| {
                let (id, _) = resolved(&expect_breakpoints(remote, "CompiledLoop"));
                for _ in 0..3 {
                    assert_eq!(expect_breakpoint_stop(remote), [id]);
                    continue_stopped(remote);
                }
            },
        );
        assert_eq!(answered(&mut interpreter, result), 2003, "one bump a pass");
        let after = interpreter.jit_statistics();
        if runs_in_machine_code(lowering) {
            assert!(before.compiled_bodies > 0, "the loop ran in machine code: {before:?}");
            assert!(
                after.freed_bodies > before.freed_bodies,
                "the patch freed the loop's code: {before:?} {after:?}"
            );
            assert!(
                after.breakpoint_exits > before.breakpoint_exits,
                "machine code exited at the patched statement: {before:?} {after:?}"
            );
        }
    }
}

ns_runtime::for_each_lowering! {
    /// A breakpoint's exits are the debugger's, not the code's failing:
    /// a compiled loop stopped at more passes than its body's exit
    /// budget allows exits is never abandoned for them.
    fn a_breakpoint_s_exits_do_not_abandon_the_body(lowering) {
        const PASSES: i64 = 100;
        let (mut interpreter, instance, _) = compiled(COMPILED_LOOP, lowering);
        assert_eq!(send_answered(&mut interpreter, &instance, "warm"), 2000);
        let before = interpreter.jit_statistics();
        let offset = line_start(COMPILED_LOOP, "self bump].");
        let remote = interpreter.attach_debugger();
        set_breakpoints(&remote, "CompiledLoop", vec![offset]);
        let ((), result, _) = debugged_by(
            remote,
            |remote| {
                let (id, _) = resolved(&expect_breakpoints(remote, "CompiledLoop"));
                for _ in 0..PASSES {
                    assert_eq!(expect_breakpoint_stop(remote), [id]);
                    continue_stopped(remote);
                }
            },
            || interpreter.send(&instance, Symbol::intern("long"), &[]),
        );
        assert_eq!(answered(&mut interpreter, result), 2000 + PASSES);
        let after = interpreter.jit_statistics();
        if runs_in_machine_code(lowering) {
            assert_eq!(
                after.abandoned_bodies, before.abandoned_bodies,
                "no body was abandoned for the breakpoint's exits: {before:?} {after:?}"
            );
            assert!(
                after.breakpoint_exits - before.breakpoint_exits >= 65,
                "machine code exited at the patch more often than a body's exit budget allows: {before:?} {after:?}"
            );
        }
    }
}

/// `loop:` sums `Helper>>helper:` over its passes, a callee its optimized
/// copy inlines, whose first statement no pass runs; `noop` does nothing.
const INLINING: &str = indoc! {"
    class Inlining = Object new () (
      public class Helper = Object new () (
        public helper: value = (
          | doubled |
          value < 0 ifTrue: [
            ^0].
          doubled:: value * 2.
          ^doubled + 1
        )
      )
      public noop = (
        ^0
      )
      public loop: turns = (
        |
          receiver = Helper new.
          sum ::= 0.
          index ::= 0.
        |
        [index < turns] whileTrue: [
          sum:: sum + (receiver helper: index).
          index:: index + 1].
        ^sum
      )
    ) : ()
"};

/// Sends `loop:` with `turns` to `instance` `times` times while `remote`'s
/// every stop is continued, asserting that each run answers `answer` and
/// stops `stops` times; answers the remote.
fn loop_stopping(
    interpreter: &mut Interpreter,
    instance: &Root,
    remote: DebugRemote,
    (times, turns, answer, stops): (usize, i64, i64, usize),
) -> DebugRemote {
    let mut remote = remote;
    for _ in 0..times {
        let (events, result, returned) =
            events_during(remote, || send_loop(interpreter, instance, turns));
        assert_eq!(stop_count(&events), stops, "{events:?}");
        assert_eq!(answered(interpreter, result), answer);
        remote = returned;
    }
    remote
}

ns_runtime::for_each_lowering! {
    /// A breakpoint in a callee an optimized copy inlined frees the copy
    /// and stops at every call; a copy compiled while it stands -- at a
    /// statement no pass runs -- calls the callee rather than inline it,
    /// and stops at every call once the breakpoint stands at one a pass
    /// runs.
    fn a_breakpoint_in_an_inlined_callee_frees_the_copy(lowering) {
        let (mut interpreter, class) = load(INLINING, lowering);
        interpreter.set_jit_stress(true);
        interpreter.set_optimize_stress(true);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..3 {
            assert_eq!(loop_answered(&mut interpreter, &instance, 100), 10000);
        }
        let selector = Symbol::intern("loop:");
        let pinned = lowering == Lowering::Optimized && counters_are_exact();
        if pinned {
            assert_eq!(
                interpreter.jit_customized_copies(&class, MethodSide::Instance, selector),
                1,
                "`loop:` runs as a copy"
            );
            assert!(
                interpreter.jit_inlined_bodies(&class, MethodSide::Instance, selector) >= 1,
                "`loop:`'s copy inlines `helper:`"
            );
        }
        let before = interpreter.jit_statistics();
        let remote = interpreter.attach_debugger();
        let answering = line_start(INLINING, "^doubled + 1");
        set_breakpoints(&remote, "Inlining", vec![answering]);
        // A run serves the command as it starts.
        assert_eq!(send_answered(&mut interpreter, &instance, "noop"), 0);
        let (id, _) = resolved(&expect_breakpoints(&remote, "Inlining"));
        if pinned {
            assert_eq!(
                interpreter.jit_customized_copies(&class, MethodSide::Instance, selector),
                0,
                "the patch freed `loop:`'s copy"
            );
            let after = interpreter.jit_statistics();
            assert!(
                after.invalidated_copies > before.invalidated_copies,
                "{before:?} {after:?}"
            );
        }
        let ((), result, remote) = debugged_by(
            remote,
            move |remote| {
                for _ in 0..3 {
                    assert_eq!(expect_breakpoint_stop(remote), [id]);
                    continue_stopped(remote);
                }
            },
            || send_loop(&mut interpreter, &instance, 3),
        );
        assert_eq!(answered(&mut interpreter, result), 9);
        // Patched at a statement no pass runs, `helper:` lets `loop:` be
        // optimized again, its copy calling `helper:`.
        set_breakpoints(&remote, "Inlining", vec![line_start(INLINING, "^0].")]);
        let remote = loop_stopping(&mut interpreter, &instance, remote, (3, 100, 10000, 0));
        if pinned {
            assert!(
                interpreter.jit_customized_copies(&class, MethodSide::Instance, selector) >= 1,
                "`loop:` is optimized again"
            );
            assert_eq!(
                interpreter.jit_inlined_bodies(&class, MethodSide::Instance, selector),
                0,
                "no copy inlines the patched callee"
            );
            let helper = expect_ok(
                interpreter.send(&instance, Symbol::intern("Helper"), &[]),
                "Helper",
            );
            assert!(
                !interpreter.jit_code_id_named_by_a_copy(
                    &helper,
                    MethodSide::Instance,
                    Symbol::intern("helper:")
                ),
                "no copy names the patched callee"
            );
        }
        // The copy's every call stops at a statement a pass runs.
        set_breakpoints(&remote, "Inlining", vec![answering]);
        loop_stopping(&mut interpreter, &instance, remote, (1, 10, 100, 10));
        if pinned {
            assert!(
                interpreter.jit_customized_copies(&class, MethodSide::Instance, selector) >= 1,
                "the copy that calls `helper:` survives its patch"
            );
        }
    }
}

/// `loop:` sums what `Helper>>apply:to:` answers for a block of its own
/// over its passes: its optimized copy inlines the callee and the block,
/// whose closure it eliminates. No pass runs the block's `0`.
const BLOCKING: &str = indoc! {"
    class Blocking = Object new () (
      public class Helper = Object new () (
        public apply: action to: value = (
          ^action value: value
        )
      )
      public loop: turns = (
        |
          receiver = Helper new.
          sum ::= 0.
          index ::= 0.
        |
        [index < turns] whileTrue: [
          sum:: sum + (receiver apply: [:value |
            value < 0
              ifTrue: [
                0]
              ifFalse: [
                value * 2]] to: index).
          index:: index + 1].
        ^sum
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A copy compiled while a breakpoint stands in a block it would
    /// inline -- at a statement no pass runs -- neither inlines nor
    /// eliminates the block, but makes its closure and runs its code, so
    /// that every pass stops once the breakpoint stands at a statement a
    /// pass runs.
    fn a_breakpoint_in_an_inlined_block_stops_in_every_copy(lowering) {
        let (mut interpreter, class) = load(BLOCKING, lowering);
        interpreter.set_jit_stress(true);
        interpreter.set_optimize_stress(true);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..3 {
            assert_eq!(loop_answered(&mut interpreter, &instance, 100), 9900);
        }
        let selector = Symbol::intern("loop:");
        let pinned = lowering == Lowering::Optimized && counters_are_exact();
        let before = interpreter.jit_statistics();
        if pinned {
            assert_eq!(
                interpreter.jit_customized_copies(&class, MethodSide::Instance, selector),
                1,
                "`loop:` runs as a copy"
            );
            assert!(before.eliminated_blocks > 0, "the copy eliminates the block: {before:?}");
        }
        let remote = interpreter.attach_debugger();
        set_breakpoints(&remote, "Blocking", vec![line_start(BLOCKING, "0]\n")]);
        let remote = loop_stopping(&mut interpreter, &instance, remote, (3, 100, 9900, 0));
        let after = interpreter.jit_statistics();
        if pinned {
            assert!(
                after.optimized_bodies > before.optimized_bodies,
                "`loop:` is optimized again: {before:?} {after:?}"
            );
            assert_eq!(
                after.eliminated_blocks, before.eliminated_blocks,
                "no copy eliminates the patched block"
            );
        }
        set_breakpoints(&remote, "Blocking", vec![line_start(BLOCKING, "value * 2]] to: index).")]);
        loop_stopping(&mut interpreter, &instance, remote, (1, 10, 90, 10));
        if pinned {
            assert!(
                interpreter.jit_customized_copies(&class, MethodSide::Instance, selector) >= 1,
                "the copy that makes the block's closure survives its patch"
            );
        }
    }
}

/// The shipped kernel's source, whose module is `Kernel`.
const KERNEL: &str = include_str!("../core-library/Kernel.ns");

/// `check:with:` sends `ifTrue:` to `true` with a block held in a
/// variable, which no lowering inlines, so that the send reaches
/// `True>>ifTrue:`, a method with a fast path.
const QUICK: &str = indoc! {"
    class Quick = Object new (
      | count ::= 0. |
    ) (
      public run = (
        | action |
        action:: [count:: count + 1].
        self check: true with: action.
        ^count
      )
      check: condition with: action = (
        ^condition ifTrue: action
      )
    ) : ()
"};

/// The fast path `True>>ifTrue:` has now.
fn if_true_fast_path(interpreter: &mut Interpreter) -> Option<ns_runtime::mixin::MethodFastPath> {
    interpreter.enter(|mutator| {
        let class = mutator.class_of(ns_runtime::Value::boolean(true));
        let mixin = mutator
            .heap()
            .class_application(class)
            .expect("True has an application")
            .mixin
            .clone();
        mixin
            .compiled_method(Symbol::intern("ifTrue:"))
            .expect("True declares ifTrue:")
            .fast_path()
    })
}

ns_runtime::for_each_lowering! {
    /// A breakpoint in a shipped method with a fast path stops: the fast
    /// path is off while it stands, so that the send activates the
    /// method. Removed at the stop, it puts the fast path back, the run
    /// goes on to its unchanged outcome, and the next run does not stop.
    fn a_breakpoint_in_a_quick_shipped_method_stops(lowering) {
        let (mut interpreter, class) = load(QUICK, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let quick = Some(ns_runtime::mixin::MethodFastPath::ValueOfParameter(0));
        assert_eq!(if_true_fast_path(&mut interpreter), quick);
        // Run often enough that `check:with:` is compiled, its cell naming
        // the stub that runs the block without activating `ifTrue:`.
        for count in 1..=5 {
            assert_eq!(send_answered(&mut interpreter, &instance, "run"), count);
        }
        let offset = line_start(KERNEL, "public ifTrue: trueBlock = (");
        let remote = interpreter.attach_debugger();
        set_breakpoints(&remote, "Kernel", vec![offset]);
        let ((statement, frames), result, remote) = debugged_by(
            remote,
            |remote| {
                let (id, statement) = resolved(&expect_breakpoints(remote, "Kernel"));
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                let frames: Vec<FrameKind> = stack_trace(remote)
                    .into_iter()
                    .take(2)
                    .map(|frame| frame.kind)
                    .collect();
                set_breakpoints(remote, "Kernel", Vec::new());
                assert_eq!(expect_breakpoints(remote, "Kernel"), []);
                continue_stopped(remote);
                (statement, frames)
            },
            || run(&mut interpreter, &instance),
        );
        assert_eq!(statement.text(KERNEL), "^trueBlock value");
        assert_eq!(frames, [method("ifTrue:"), method("check:with:")]);
        assert_eq!(answered(&mut interpreter, result), 6);
        assert_eq!(if_true_fast_path(&mut interpreter), quick);
        let (events, result, _) = events_during(remote, || run(&mut interpreter, &instance));
        assert_no_stop(&events);
        assert_eq!(answered(&mut interpreter, result), 7);
    }
}

/// `loop:` sends `bump` once a pass, whose one statement a breakpoint
/// stands at: `warm` runs 2,000 passes, `run` three.
const BUMPED: &str = indoc! {"
    class Bumped = Object new (
      | count ::= 0. |
    ) (
      public warm = (
        ^self loop: 2000
      )
      public run = (
        ^self loop: 3
      )
      loop: turns = (
        1 to: turns do: [:index |
          self bump].
        ^count
      )
      bump = (
        count:: count + 1
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A body a breakpoint patched is compiled again once the breakpoint
    /// is removed: neither the exit at the patch nor the code freed for it
    /// leaves the body interpreted for good.
    fn a_removed_breakpoint_lets_the_body_compile_again(lowering) {
        let (mut interpreter, instance, class) = compiled(BUMPED, lowering);
        let bump = Symbol::intern("bump");
        assert_eq!(send_answered(&mut interpreter, &instance, "warm"), 2000);
        if runs_in_machine_code(lowering) {
            assert!(
                interpreter.jit_body_offset(&class, MethodSide::Instance, bump).is_some(),
                "`bump` is compiled"
            );
        }
        let remote = interpreter.attach_debugger();
        set_breakpoints(&remote, "Bumped", vec![line_start(BUMPED, "count:: count + 1")]);
        let ((), result, remote) = debugged_by(
            remote,
            |remote| {
                let (id, _) = resolved(&expect_breakpoints(remote, "Bumped"));
                for _ in 0..3 {
                    assert_eq!(expect_breakpoint_stop(remote), [id]);
                    continue_stopped(remote);
                }
            },
            || run(&mut interpreter, &instance),
        );
        assert_eq!(answered(&mut interpreter, result), 2003);
        set_breakpoints(&remote, "Bumped", Vec::new());
        let (events, result, _) = events_during(remote, || {
            interpreter.send(&instance, Symbol::intern("warm"), &[])
        });
        assert_no_stop(&events);
        assert!(reports_no_breakpoint(&events, "Bumped"), "{events:?}");
        assert_eq!(answered(&mut interpreter, result), 4003);
        if runs_in_machine_code(lowering) {
            assert!(
                interpreter.jit_body_offset(&class, MethodSide::Instance, bump).is_some(),
                "`bump` is compiled again"
            );
        }
    }
}

/// Sends `selector` to an instance of `source`'s module class under
/// `lowering`, its JIT on, with a debugger attached: the selector is sent
/// twice before, so that the bodies it runs have machine code, which a
/// step frees; `configure` queues its commands before the run, and
/// `agent` serves it from another thread. Answers what `agent` answered
/// and the run's answer.
fn stepped<T: Send + 'static>(
    source: &str,
    lowering: Lowering,
    selector: &'static str,
    configure: impl FnOnce(&DebugRemote),
    agent: impl FnOnce(&DebugRemote) -> T + Send + 'static,
) -> (T, i64) {
    stepped_with(source, lowering, true, selector, configure, agent)
}

/// `stepped`, the JIT on or off: off, every body runs interpreted, so
/// that a register holds what the interpreter left there, which machine
/// code, writing a send's arguments into its slots, would not.
fn stepped_with<T: Send + 'static>(
    source: &str,
    lowering: Lowering,
    jit: bool,
    selector: &'static str,
    configure: impl FnOnce(&DebugRemote),
    agent: impl FnOnce(&DebugRemote) -> T + Send + 'static,
) -> (T, i64) {
    let (mut interpreter, instance) = instance_of(source, lowering);
    interpreter.set_jit_enabled(jit);
    for _ in 0..2 {
        expect_ok(
            interpreter.send(&instance, Symbol::intern(selector), &[]),
            "warm",
        );
    }
    let remote = interpreter.attach_debugger();
    configure(&remote);
    let (answer, result, _) = debugged_by(remote, agent, || {
        interpreter.send(&instance, Symbol::intern(selector), &[])
    });
    let answered = answered(&mut interpreter, result);
    (answer, answered)
}

/// Sends the step `command` to the stopped program and waits for its
/// stop: the confirmation first, then the step's stop, whose returned
/// value it answers.
fn step(remote: &DebugRemote, command: Command) -> Option<ValueDescription> {
    remote.send(command);
    match remote.next_event(EVENT_TIMEOUT) {
        Some(Event::Continued) => {}
        other => panic!("the confirmation of the step was expected, not {other:?}"),
    }
    match remote.next_event(EVENT_TIMEOUT) {
        Some(Event::Stopped {
            reason: StopReason::Step,
            returned,
        }) => returned,
        other => panic!("a stop of the step was expected, not {other:?}"),
    }
}

/// Waits for the answer to `SetBreakpoints` for `module` and for the stop
/// at its one breakpoint.
fn expect_first_stop(remote: &DebugRemote, module: &str) {
    let (id, _) = resolved(&expect_breakpoints(remote, module));
    assert_eq!(expect_breakpoint_stop(remote), [id]);
}

/// The name of the stopped top frame and the text of its statement in
/// `source`.
fn standing(remote: &DebugRemote, source: &str) -> (String, String) {
    let frames = stack_trace(remote);
    let top = &frames[0];
    let location = top
        .location
        .clone()
        .unwrap_or_else(|| panic!("{} is located", top.name));
    (top.name.clone(), location.statement.text(source).to_owned())
}

/// The label of the value of the variable named `name` in the stopped
/// top frame.
fn top_variable(remote: &DebugRemote, name: &str) -> String {
    let variables = variables(remote, Command::Variables { frame: 0 });
    value_of(&variables, name).label.clone()
}

/// A method whose first statement sends a method of three statements, one
/// that answers that method's answer plus one, one whose first statement
/// only a primitive answers, and one that sends a method whose block
/// writes its temporary, which lives in an indirection vector.
const STEPPING: &str = indoc! {"
    class Stepping = Object new (
      | count ::= 0. |
    ) (
      public run = (
        | x |
        x:: self compute.
        count:: count + x.
        ^count
      )
      public calling = (
        ^self compute + 1
      )
      public arithmetic = (
        | y |
        y:: 3 + 4.
        ^y * 2
      )
      public vectoring = (
        ^self captured
      )
      compute = (
        | a |
        a:: 3.
        a:: a + 4.
        ^a * 2
      )
      captured = (
        | b |
        b:: 5.
        [b:: b + 1] value.
        ^b
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step over at a statement that sends stops at the next statement
    /// of the same frame, past every statement of the callee.
    fn a_step_over_stops_at_the_next_statement(lowering) {
        let offset = line_start(STEPPING, "x:: self compute.");
        let (stood, _) = stepped(
            STEPPING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Stepping", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Stepping");
                step(remote, Command::StepOver);
                let stood = standing(remote, STEPPING);
                continue_stopped(remote);
                stood
            },
        );
        assert_eq!(
            stood,
            (String::from("Stepping>>run"), String::from("count:: count + x"))
        );
    }
}

/// `stops` as pairs of string slices, to compare with a literal.
fn as_pairs(stops: &[(String, String)]) -> Vec<(&str, &str)> {
    stops
        .iter()
        .map(|(frame, statement)| (frame.as_str(), statement.as_str()))
        .collect()
}

/// A module whose slot initializers and statement run in the host run of
/// its instantiation, a run of its own before the one that sends `main`, as
/// `ns-next run` runs a root's instantiation before `main:args:`.
const TWO_RUNS: &str = indoc! {"
    class TwoRuns = Object new (
      |
        first ::= 1.
        second ::= first + 1.
      |
      second:: second * 10.
    ) (
      public main = (
        | x |
        x:: second + 1.
        ^x
      )
    ) : ()
"};

/// Instantiates `TWO_RUNS`'s module class under `lowering`, a breakpoint
/// at its last slot initializer, and sends the instance `main`, a debugger
/// attached whose `agent` serves both runs from another thread. Answers
/// what `agent` answered and `main`'s answer.
fn across_two_runs<T: Send + 'static>(
    lowering: Lowering,
    agent: impl FnOnce(&DebugRemote) -> T + Send + 'static,
) -> (T, i64) {
    let (mut interpreter, class) = load(TWO_RUNS, lowering);
    let remote = interpreter.attach_debugger();
    let offset = line_start(TWO_RUNS, "second ::= first + 1.");
    set_breakpoints(&remote, "TwoRuns", vec![offset]);
    let (answer, result, _) = debugged_by(remote, agent, || {
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        interpreter.send(&instance, Symbol::intern("main"), &[])
    });
    (answer, answered(&mut interpreter, result))
}

ns_runtime::for_each_lowering! {
    /// A step whose frame leaves its host run -- the instantiation's
    /// initializer, which returns to the host -- goes on in the program's
    /// next run, the next place the program reaches (decision 17): the
    /// run's entry stops at its first statement, a step's stop, whether
    /// the step was over, into or out. A step over or into from the last
    /// slot initializer stops at the initializer's statement first; none
    /// stops in the factory, which is passed over (decision 12).
    fn a_step_that_leaves_its_run_stops_at_the_next_run_s_first_statement(lowering) {
        for command in [Command::StepOver, Command::StepInto, Command::StepOut] {
            let name = command.name();
            let out = command == Command::StepOut;
            let (stops, answer) = across_two_runs(lowering, move |remote| {
                expect_first_stop(remote, "TwoRuns");
                let mut stops = Vec::new();
                while stops.len() < 4 {
                    step(remote, command.clone());
                    let stood = standing(remote, TWO_RUNS);
                    let in_main = stood.0 == "TwoRuns>>main";
                    stops.push(stood);
                    if in_main {
                        break;
                    }
                }
                continue_stopped(remote);
                stops
            });
            let mut expected = vec![("TwoRuns>>main", "x:: second + 1")];
            if !out {
                expected.insert(0, ("TwoRuns initializer", "second:: second * 10"));
            }
            assert_eq!(as_pairs(&stops), expected, "{name}");
            assert_eq!(answer, 21, "{name}");
        }
    }
}

ns_runtime::for_each_lowering! {
    /// A `Terminate` sent between runs ends the step its run left, as it
    /// would end the step in a run: the next run stops nowhere, and the
    /// `Terminate`, which met no run, is refused.
    fn a_terminate_between_runs_drops_the_step_its_run_left(lowering) {
        let (mut interpreter, class) = load(TWO_RUNS, lowering);
        let remote = interpreter.attach_debugger();
        let offset = line_start(TWO_RUNS, "second ::= first + 1.");
        set_breakpoints(&remote, "TwoRuns", vec![offset]);
        let agent = |remote: &DebugRemote| {
            expect_first_stop(remote, "TwoRuns");
            // The first step stops at the initializer's statement; the
            // second leaves the run, past the factory.
            step(remote, Command::StepOver);
            remote.send(Command::StepOver);
            match remote.next_event(EVENT_TIMEOUT) {
                Some(Event::Continued) => {}
                other => panic!("the confirmation of the step was expected, not {other:?}"),
            }
            // The step leaves the run without a stop.
            while !run_finished() {
                if let Some(event) = remote.next_event(Duration::from_millis(10)) {
                    panic!("the instantiation was expected to end silently, not with {event:?}");
                }
            }
        };
        let (_, result, remote) =
            debugged_by(remote, agent, || interpreter.instantiate(&class, &[]));
        let instance = expect_ok(result, "instantiate");
        remote.send(Command::Terminate);
        let (events, result, remote) = events_during(remote, || {
            interpreter.send(&instance, Symbol::intern("main"), &[])
        });
        assert_no_stop(&events);
        assert!(
            events.iter().any(|event| matches!(
                event,
                Event::Refused { command, .. } if command == "Terminate"
            )),
            "{events:?}"
        );
        assert_eq!(answered(&mut interpreter, result), 21);
        drop(remote);
    }
}

/// Classes instantiated within a run: one whose initializer has slot
/// initializers and statements, a subclass whose superclass's
/// initializer has statements, a class that applies a mixin by a `<:`
/// clause, and one whose superclass clause's argument sends a method of
/// the module.
const INITIALIZING: &str = indoc! {"
    class Initializing = Object new () (
      class Header = (
        |
          first ::= 1.
          public second ::= first + 1.
        |
        first:: first + 10.
        second:: second + 10.
      ) ()
      class Base = (
        |
          a ::= 1.
        |
        a:: a + 1.
      ) ()
      class Derived = Base new (
        |
          public b ::= 2.
        |
        b:: b + 1.
      ) ()
      class Extra = (
        |
          public c ::= 3.
        |
        c:: c + 1.
      ) ()
      class Mixed = Base new <: Extra new (
        |
          public d ::= 4.
        |
      ) ()
      class Setup withSetup: value = (
        |
          public e ::= value.
        |
      ) ()
      class Clause = Setup withSetup: setupValue (
        |
          public f ::= 6.
        |
      ) ()
      class Ending = (
        |
          public g ::= 0.
        |
        g < 5 ifTrue: [
          g:: g + 1.
          g:: g + 2].
      ) ()
      class Taker new: block = (
        |
          public h ::= block value.
        |
      ) ()
      setupValue = (
        ^5
      )
      public ending = (
        | made |
        made:: Ending new.
        ^made g
      )
      public taking = (
        | made total |
        total:: 0.
        made:: Taker new: [
          total:: total + 1.
          total + 40].
        ^made h
      )
      public header = (
        | made |
        made:: Header new.
        ^made second
      )
      public derived = (
        | made |
        made:: Derived new.
        ^made b
      )
      public mixed = (
        | made |
        made:: Mixed new.
        ^made c + made d
      )
      public clause = (
        | made |
        made:: Clause new.
        ^made e + made f
      )
    ) : ()
"};

/// A module whose initializer's last statement is an inlined form, run in
/// the host run of its instantiation before the one that sends `main`.
const ENDING_RUNS: &str = indoc! {"
    class EndingRuns = Object new (
      |
        first ::= 1.
      |
      first > 0 ifTrue: [
        first:: first + 1.
        first:: first * 10].
    ) (
      public main = (
        | x |
        x:: first + 1.
        ^x
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step out of an inlined form that is the last statement of the
    /// root's initializer leaves the instantiation's run, past the
    /// factory, and goes on in the next run: the step carried stops at
    /// `main`'s first statement.
    fn a_step_out_of_an_initializer_s_last_form_goes_on_in_the_next_run(lowering) {
        let (mut interpreter, class) = load(ENDING_RUNS, lowering);
        let remote = interpreter.attach_debugger();
        let offset = line_start(ENDING_RUNS, "first:: first + 1.");
        set_breakpoints(&remote, "EndingRuns", vec![offset]);
        let agent = |remote: &DebugRemote| {
            expect_first_stop(remote, "EndingRuns");
            step(remote, Command::StepOut);
            let stood = standing(remote, ENDING_RUNS);
            continue_stopped(remote);
            stood
        };
        let (stood, result, _) = debugged_by(remote, agent, || {
            let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
            interpreter.send(&instance, Symbol::intern("main"), &[])
        });
        assert_eq!(
            stood,
            (String::from("EndingRuns>>main"), String::from("x:: first + 1"))
        );
        assert_eq!(answered(&mut interpreter, result), 21);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of an inlined form that is an initializer's last
    /// statement, in a run's `new`, leaves the initializer, passes over the
    /// factory (decision 12) and stops at the caller's place after the
    /// `new`.
    fn a_step_out_of_an_initializer_s_last_form_stops_after_the_new(lowering) {
        let (stops, answer) = initializing_stops(
            lowering,
            "ending",
            "g:: g + 1.",
            Command::StepOut,
            ("Initializing>>ending", "made:: Ending new"),
        );
        assert_eq!(
            as_pairs(&stops),
            [("Initializing>>ending", "made:: Ending new")]
        );
        assert_eq!(answer, 3);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of a block handed to a factory and run by a slot
    /// initializer leaves the send of the factory, which took it, past
    /// the initializer and the factory: it stops after the `new:`, in the
    /// statement that assigns what it answered.
    fn a_step_out_of_a_block_a_slot_initializer_runs_stops_after_the_factory_s_send(
        lowering
    ) {
        let statement = {
            let from = INITIALIZING.find("made:: Taker new: [").expect("the statement");
            let until = INITIALIZING[from..].find("total + 40]").expect("its end") + from;
            &INITIALIZING[from..until + "total + 40]".len()]
        };
        let (stops, answer) = initializing_stops(
            lowering,
            "taking",
            "total:: total + 1.",
            Command::StepOut,
            ("Initializing>>taking", statement),
        );
        assert_eq!(as_pairs(&stops), [("Initializing>>taking", statement)]);
        assert_eq!(answer, 41);
    }
}

/// Sends `selector` to an instance of `INITIALIZING` under `lowering`, a
/// breakpoint on the line of `breakpoint`, and from its stop takes
/// `command` until the program stands at `last`, at most twelve times:
/// answers every stop the steps made, as its frame's name and statement,
/// and the run's answer.
fn initializing_stops(
    lowering: Lowering,
    selector: &'static str,
    breakpoint: &str,
    command: Command,
    last: (&'static str, &'static str),
) -> (Vec<(String, String)>, i64) {
    let offset = line_start(INITIALIZING, breakpoint);
    stepped(
        INITIALIZING,
        lowering,
        selector,
        |remote| set_breakpoints(remote, "Initializing", vec![offset]),
        move |remote| {
            expect_first_stop(remote, "Initializing");
            let mut stops = Vec::new();
            while stops.len() < 12 {
                step(remote, command.clone());
                let stood = standing(remote, INITIALIZING);
                let done = stood.0 == last.0 && stood.1 == last.1;
                stops.push(stood);
                if done {
                    break;
                }
            }
            continue_stopped(remote);
            stops
        },
    )
}

ns_runtime::for_each_lowering! {
    /// A step over goes through every slot initializer of an initializer
    /// and then each of its statements, and from the last one, past the
    /// factory (decision 12), to the caller's place after the `new`.
    fn a_step_over_goes_through_an_initializer_and_past_the_factory(lowering) {
        let (stops, answer) = initializing_stops(
            lowering,
            "header",
            "first ::= 1.",
            Command::StepOver,
            ("Initializing>>header", "^made second"),
        );
        assert_eq!(
            as_pairs(&stops),
            [
                ("Header initializer", "public second ::= first + 1."),
                ("Header initializer", "first:: first + 10"),
                ("Header initializer", "second:: second + 10"),
                ("Initializing>>header", "made:: Header new"),
                ("Initializing>>header", "^made second"),
            ]
        );
        assert_eq!(answer, 12);
    }
}

ns_runtime::for_each_lowering! {
    /// A step over at the last statement of a superclass's initializer
    /// stops at the first statement of the next level's, the
    /// subclass's, never in the factory.
    fn a_step_over_from_a_superclass_s_initializer_stops_in_the_next_level_s(lowering) {
        let (stops, answer) = initializing_stops(
            lowering,
            "derived",
            "a:: a + 1.",
            Command::StepOver,
            ("Initializing>>derived", "^made b"),
        );
        assert_eq!(
            as_pairs(&stops),
            [
                ("Derived initializer", "public b ::= 2."),
                ("Derived initializer", "b:: b + 1"),
                ("Initializing>>derived", "made:: Derived new"),
                ("Initializing>>derived", "^made b"),
            ]
        );
        assert_eq!(answer, 3);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out taken in a superclass's initializer leaves its frame and
    /// stops at the first statement of the next level's initializer, the
    /// subclass's; one taken there leaves the last level's initializer,
    /// passes over the factory (decision 12), and stops at the caller's
    /// place after the `new`.
    fn a_step_out_of_an_initializer_stops_in_the_next_level_s_then_after_the_new(lowering) {
        let (stops, answer) = initializing_stops(
            lowering,
            "derived",
            "a ::= 1.",
            Command::StepOut,
            ("Initializing>>derived", "made:: Derived new"),
        );
        assert_eq!(
            as_pairs(&stops),
            [
                ("Derived initializer", "public b ::= 2."),
                ("Initializing>>derived", "made:: Derived new"),
            ]
        );
        assert_eq!(answer, 3);
    }
}

ns_runtime::for_each_lowering! {
    /// A step into from an instantiation whose class applies a mixin by a
    /// `<:` clause goes through every level's initializer in the order
    /// the program runs them (§6.1, §6.2): the check of the `<:` clause,
    /// the superclass clause and the superclass's initializer, from its
    /// own superclass clause on, the `<:` clause's message and the mixin's
    /// initializer, then the class's own slot.
    fn a_step_into_goes_through_a_mixin_application_s_initializers_in_order(lowering) {
        let (stops, answer) = initializing_stops(
            lowering,
            "mixed",
            "made:: Mixed new.",
            Command::StepInto,
            ("Initializing>>mixed", "^made c + made d"),
        );
        assert_eq!(
            as_pairs(&stops),
            [
                ("Mixed initializer", "Extra new"),
                ("Mixed initializer", "Base new"),
                ("Base initializer", "Base"),
                ("Base initializer", "a ::= 1."),
                ("Base initializer", "a:: a + 1"),
                ("Mixed initializer", "Extra new"),
                ("Extra initializer", "public c ::= 3."),
                ("Extra initializer", "c:: c + 1"),
                ("Mixed initializer", "public d ::= 4."),
                ("Initializing>>mixed", "made:: Mixed new"),
                ("Initializing>>mixed", "^made c + made d"),
            ]
        );
        assert_eq!(answer, 8);
    }
}

ns_runtime::for_each_lowering! {
    /// A step into from an instantiation whose superclass clause's
    /// argument sends a method of the module enters that method where the
    /// program runs it, in the superclass clause, and goes on through the
    /// rest of the clause, the superclass's initializer and the class's
    /// own.
    fn a_step_into_enters_a_method_a_superclass_clause_sends(lowering) {
        let (stops, answer) = initializing_stops(
            lowering,
            "clause",
            "made:: Clause new.",
            Command::StepInto,
            ("Initializing>>clause", "^made e + made f"),
        );
        assert_eq!(
            as_pairs(&stops),
            [
                ("Clause initializer", "Setup withSetup: setupValue"),
                ("Initializing>>setupValue", "^5"),
                ("Clause initializer", "Setup withSetup: setupValue"),
                ("Setup initializer", "Setup"),
                ("Setup initializer", "public e ::= value."),
                ("Clause initializer", "public f ::= 6."),
                ("Initializing>>clause", "made:: Clause new"),
                ("Initializing>>clause", "^made e + made f"),
            ]
        );
        assert_eq!(answer, 11);
    }
}

ns_runtime::for_each_lowering! {
    /// A step into at a statement that sends a method stops at the
    /// callee's first statement, the callee on top of the stack.
    fn a_step_into_stops_in_the_callee_s_first_statement(lowering) {
        let offset = line_start(STEPPING, "x:: self compute.");
        let ((stood, names), _) = stepped(
            STEPPING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Stepping", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Stepping");
                assert_eq!(step(remote, Command::StepInto), None);
                let stood = standing(remote, STEPPING);
                let frames = stack_trace(remote);
                let names: Vec<String> =
                    frames.iter().map(|frame| frame.name.clone()).collect();
                continue_stopped(remote);
                (stood, names)
            },
        );
        assert_eq!(
            stood,
            (String::from("Stepping>>compute"), String::from("a:: 3"))
        );
        assert_eq!(names, ["Stepping>>compute", "Stepping>>run"]);
    }
}

/// A method whose first statement sends a method with an empty body, and
/// one whose statement hands a block to the shipped `do:`, a block whose
/// only statement names its parameter -- a statement the optimized
/// lowering compiles to no instruction, since the block answers the
/// parameter's register.
const EMPTY_BODIES: &str = indoc! {"
    class Emptied = Object new () (
      public run = (
        | x |
        x:: self nothing.
        ^3
      )
      public each = (
        {1. 2} do: [:each | each].
        ^4
      )
      nothing = ()
    ) : ()
"};

/// Sends `selector` to an instance of `EMPTY_BODIES` under `lowering`, a
/// breakpoint on the line of `breakpoint`, and takes one step into from
/// its stop: answers where it stopped, its frame's name and statement, and
/// the names of the frames on the stack.
fn stepped_into_empty(
    lowering: Lowering,
    selector: &'static str,
    breakpoint: &str,
) -> ((String, String), Vec<String>) {
    let offset = line_start(EMPTY_BODIES, breakpoint);
    let (stopped, _) = stepped(
        EMPTY_BODIES,
        lowering,
        selector,
        |remote| set_breakpoints(remote, "Emptied", vec![offset]),
        |remote| {
            expect_first_stop(remote, "Emptied");
            step(remote, Command::StepInto);
            let stood = standing(remote, EMPTY_BODIES);
            let names: Vec<String> = stack_trace(remote)
                .iter()
                .map(|frame| frame.name.clone())
                .collect();
            continue_stopped(remote);
            (stood, names)
        },
    );
    stopped
}

ns_runtime::for_each_lowering! {
    /// A step into a method whose body is empty stops in it, standing in
    /// the body, as in any method it pushes: only a factory, known by its
    /// frame's kind, is passed over (decision 12), never a body for having
    /// no statement start.
    fn a_step_into_an_empty_body_stops_in_it(lowering) {
        let (stood, names) = stepped_into_empty(lowering, "run", "x:: self nothing.");
        assert_eq!(
            stood,
            (String::from("Emptied>>nothing"), String::from("nothing = ()"))
        );
        assert_eq!(names, ["Emptied>>nothing", "Emptied>>run"]);
    }
}

ns_runtime::for_each_lowering! {
    /// A step into a block whose only statement names its parameter stops
    /// in the block under both lowerings, at that statement, though the
    /// optimized one compiles it to no instruction: the return the block
    /// ends with, which no statement holds, stands at the statement it
    /// follows.
    fn a_step_into_a_block_naming_its_parameter_stops_in_it(lowering) {
        let ((frame, statement), names) =
            stepped_into_empty(lowering, "each", "{1. 2} do: [:each | each].");
        assert_eq!(frame, "[] in Emptied>>each");
        assert_eq!(statement, "each");
        assert_eq!(names.first().map(String::as_str), Some("[] in Emptied>>each"));
        assert_eq!(names.last().map(String::as_str), Some("Emptied>>each"));
    }
}

ns_runtime::for_each_lowering! {
    /// A step into at a statement whose send a primitive answers, which
    /// pushes nothing, stops at the next statement.
    fn a_step_into_a_primitive_answered_send_stops_at_the_next_statement(lowering) {
        let offset = line_start(STEPPING, "y:: 3 + 4.");
        let (stood, answer) = stepped(
            STEPPING,
            lowering,
            "arithmetic",
            |remote| set_breakpoints(remote, "Stepping", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Stepping");
                step(remote, Command::StepInto);
                let stood = standing(remote, STEPPING);
                continue_stopped(remote);
                stood
            },
        );
        assert_eq!(
            stood,
            (String::from("Stepping>>arithmetic"), String::from("^y * 2"))
        );
        assert_eq!(answer, 14);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out stops in the caller at the index after its send,
    /// mid-statement, and reports what the callee answered.
    fn a_step_out_stops_at_the_return_point(lowering) {
        let offset = line_start(STEPPING, "a:: a + 4.");
        let ((stood, returned), answer) = stepped(
            STEPPING,
            lowering,
            "calling",
            |remote| set_breakpoints(remote, "Stepping", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Stepping");
                let returned = step(remote, Command::StepOut);
                let stood = standing(remote, STEPPING);
                continue_stopped(remote);
                (stood, returned)
            },
        );
        assert_eq!(
            stood,
            (
                String::from("Stepping>>calling"),
                String::from("^self compute + 1")
            )
        );
        assert_eq!(returned.expect("the callee's answer").label, "14");
        assert_eq!(answer, 15);
    }
}

ns_runtime::for_each_lowering! {
    /// A step over a statement that assigns a send's answer reports that
    /// answer as the value of the last call.
    fn a_step_over_reports_the_returned_value(lowering) {
        let offset = line_start(STEPPING, "x:: self compute.");
        let (returned, _) = stepped(
            STEPPING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Stepping", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Stepping");
                let returned = step(remote, Command::StepOver);
                continue_stopped(remote);
                returned
            },
        );
        let returned = returned.expect("compute's answer");
        assert_eq!(returned.label, "14");
        assert_eq!(returned.class_name, "Integer");
    }
}

ns_runtime::for_each_lowering! {
    /// A step over at a method's last statement stops at its caller's
    /// return point, and reports what the method answered.
    fn a_step_over_at_the_last_statement_stops_in_the_caller(lowering) {
        let offset = line_start(STEPPING, "^a * 2");
        let ((stood, returned), answer) = stepped(
            STEPPING,
            lowering,
            "calling",
            |remote| set_breakpoints(remote, "Stepping", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Stepping");
                let returned = step(remote, Command::StepOver);
                let stood = standing(remote, STEPPING);
                continue_stopped(remote);
                (stood, returned)
            },
        );
        assert_eq!(
            stood,
            (
                String::from("Stepping>>calling"),
                String::from("^self compute + 1")
            )
        );
        assert_eq!(returned.expect("compute's answer").label, "14");
        assert_eq!(answer, 15);
    }
}

/// A factorial that keeps the recursive call's answer in a temporary,
/// a statement of its own before the one that answers it.
const RECURSIVE: &str = indoc! {"
    class Recursive = Object new () (
      public run = (
        ^self factorial: 5
      )
      factorial: n = (
        | result |
        n <= 1 ifTrue: [^1].
        result:: n * (self factorial: n - 1).
        ^result
      )
    ) : ()
"};

/// How many frames of the stopped stack run `factorial:`.
fn factorial_frames(remote: &DebugRemote) -> usize {
    stack_trace(remote)
        .iter()
        .filter(|frame| frame.kind == method("factorial:"))
        .count()
}

ns_runtime::for_each_lowering! {
    /// A step over a recursive send stops in the activation it was taken
    /// in, not in a deeper one of the same body, which runs past its
    /// one-shots; a step out of an activation stopped at a breakpoint
    /// stops in its caller.
    fn a_step_over_a_recursive_call_stays_in_its_activation(lowering) {
        let recursing = line_start(RECURSIVE, "result:: n * (self factorial: n - 1).");
        let answering = line_start(RECURSIVE, "^result");
        let (seen, answer) = stepped(
            RECURSIVE,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Recursive", vec![recursing]),
            move |remote| {
                let (id, _) = resolved(&expect_breakpoints(remote, "Recursive"));
                for _ in 0..2 {
                    assert_eq!(expect_breakpoint_stop(remote), [id]);
                    continue_stopped(remote);
                }
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                assert_eq!(factorial_frames(remote), 3);
                // The breakpoint goes, so that only the step stops.
                set_breakpoints(remote, "Recursive", Vec::new());
                assert_eq!(expect_breakpoints(remote, "Recursive"), []);
                step(remote, Command::StepOver);
                let over = (
                    standing(remote, RECURSIVE),
                    factorial_frames(remote),
                    top_variable(remote, "n"),
                );
                // Set where the frame stands, the breakpoint lets it past
                // and stops in its caller, which a step out leaves for
                // its own caller's return point.
                set_breakpoints(remote, "Recursive", vec![answering]);
                let (id, _) = resolved(&expect_breakpoints(remote, "Recursive"));
                continue_stopped(remote);
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                let caller = (factorial_frames(remote), top_variable(remote, "n"));
                let returned = step(remote, Command::StepOut);
                let out = (
                    standing(remote, RECURSIVE),
                    factorial_frames(remote),
                    top_variable(remote, "n"),
                    returned.map(|returned| returned.label),
                );
                set_breakpoints(remote, "Recursive", Vec::new());
                expect_breakpoints(remote, "Recursive");
                continue_stopped(remote);
                (over, caller, out)
            },
        );
        let (over, caller, out) = seen;
        let factorial = String::from("Recursive>>factorial:");
        assert_eq!(
            over,
            ((factorial.clone(), String::from("^result")), 3, String::from("3"))
        );
        assert_eq!(caller, (2, String::from("4")));
        assert_eq!(
            out,
            (
                (
                    factorial,
                    String::from("result:: n * (self factorial: n - 1)")
                ),
                1,
                String::from("5"),
                Some(String::from("24")),
            )
        );
        assert_eq!(answer, 120);
    }
}

/// An `ifTrue:` sent to an object that answers it by running its
/// argument, the fallback closure of the block the compiler inlines.
const FALLING: &str = indoc! {"
    class Falling = Object new (
      | count ::= 0. |
    ) (
      public run = (
        ^self check: Yes new
      )
      check: condition = (
        count:: 1.
        condition ifTrue: [
          count:: count + 10].
        ^count
      )
      public class Yes = Object new () (
        public ifTrue: block = (
          ^block value
        )
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step over an `ifTrue:` sent to an object that is no Boolean stops
    /// in its block, which runs as the fallback closure, as it stops in
    /// the inline copy for a Boolean; a step over the block's last
    /// statement goes on in the method that created it.
    fn a_step_over_a_non_boolean_if_true_stops_in_its_fallback_block(lowering) {
        let offset = line_start(FALLING, "condition ifTrue: [");
        let ((inside, after), answer) = stepped(
            FALLING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Falling", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Falling");
                step(remote, Command::StepOver);
                let inside = standing(remote, FALLING);
                step(remote, Command::StepOver);
                let after = standing(remote, FALLING);
                continue_stopped(remote);
                (inside, after)
            },
        );
        assert_eq!(
            inside,
            (
                String::from("[] in Falling>>check:"),
                String::from("count:: count + 10")
            )
        );
        assert_eq!(
            after,
            (String::from("Falling>>check:"), String::from("^count"))
        );
        assert_eq!(answer, 11);
    }
}

ns_runtime::for_each_lowering! {
    /// A step into taken in a fallback block steps in the block's
    /// creator: the receiver's `ifTrue:` the block returns to was on the
    /// stack already and is no push, so the step goes on to the creator's
    /// next statement.
    fn a_step_into_from_a_fallback_block_goes_on_in_its_creator(lowering) {
        let offset = line_start(FALLING, "count:: count + 10].");
        let (after, answer) = stepped(
            FALLING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Falling", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Falling");
                step(remote, Command::StepInto);
                let after = standing(remote, FALLING);
                continue_stopped(remote);
                after
            },
        );
        assert_eq!(
            after,
            (String::from("Falling>>check:"), String::from("^count"))
        );
        assert_eq!(answer, 11);
    }
}

/// A loop whose condition and body only primitives answer.
const COUNTED: &str = indoc! {"
    class Counted = Object new () (
      public run = (
        | index total |
        index:: 0.
        total:: 0.
        [index < 2] whileTrue: [
          index:: index + 1.
          total:: total + index].
        ^total
      )
    ) : ()
"};

/// The statements `command` stops at, one step after another, from the
/// breakpoint at `COUNTED`'s `total:: 0.` until `^total`, and the run's
/// answer.
fn loop_stops(lowering: Lowering, command: Command) -> (Vec<String>, i64) {
    let offset = line_start(COUNTED, "total:: 0.");
    stepped(
        COUNTED,
        lowering,
        "run",
        |remote| set_breakpoints(remote, "Counted", vec![offset]),
        move |remote| {
            expect_first_stop(remote, "Counted");
            let mut statements = Vec::new();
            loop {
                step(remote, command.clone());
                let (_, statement) = standing(remote, COUNTED);
                let done = statement == "^total";
                statements.push(statement);
                if done || statements.len() > 20 {
                    break;
                }
            }
            continue_stopped(remote);
            statements
        },
    )
}

/// The statements a step over or a step into stops at in `COUNTED`'s
/// loop, under either lowering: the loop's own statement, which starts
/// before its condition's, then each of the condition's and the body's
/// once a pass.
const COUNTED_STOPS: [&str; 9] = [
    "[index < 2] whileTrue: [
      index:: index + 1.
      total:: total + index]",
    "index < 2",
    "index:: index + 1",
    "total:: total + index",
    "index < 2",
    "index:: index + 1",
    "total:: total + index",
    "index < 2",
    "^total",
];

ns_runtime::for_each_lowering! {
    /// Steps over a loop stop at each statement of its condition and its
    /// body once per pass, the loop's inlined statements flooded, and
    /// leave it for the statement after it.
    fn a_step_over_a_loop_with_no_send_does_not_stop_inside_it_more_than_once_per_statement(lowering) {
        let (statements, answer) = loop_stops(lowering, Command::StepOver);
        assert_eq!(statements, COUNTED_STOPS);
        assert_eq!(answer, 3);
    }
}

ns_runtime::for_each_lowering! {
    /// Steps into a loop with no send stop where steps over it stop: a
    /// step into taken in the loop's body, which the reference lowering
    /// runs as a block, steps in the method, and no frame between the two
    /// is taken for a push.
    fn a_step_into_a_loop_with_no_send_stops_where_a_step_over_does(lowering) {
        let (statements, answer) = loop_stops(lowering, Command::StepInto);
        assert_eq!(statements, COUNTED_STOPS);
        assert_eq!(answer, 3);
    }
}

/// Blocks a send takes, inlined and not, and blocks run from elsewhere
/// than that send: what a step out of a block leaves (decision 17).
const LEAVING: &str = indoc! {"
    class Leaving = Object new (
      | kept |
    ) (
      class Truth = () (
        public ifTrue: block = (
          ^block value
        )
      )
      class Runner = (
        | kept |
      ) (
        public keep: block = (
          kept:: block
        )
        public run: value = (
          ^kept value: value
        )
      )
      class Proxy on: subject = (
        | target = subject. |
      ) (
        public doesNotUnderstand: message = (
          ^message sendTo: target
        )
      )
      public looping = (
        | index total |
        index:: 0.
        total:: 0.
        [index < 5] whileTrue: [
          index:: index + 1.
          total:: total + index].
        ^total
      )
      public conditional = (
        | x |
        x:: 1.
        x > 0 ifTrue: [
          x:: x + 10.
          x:: x + 100].
        ^x
      )
      public nesting = (
        | index hits |
        index:: 0.
        hits:: 0.
        [index < 3] whileTrue: [
          index:: index + 1.
          index > 1 ifTrue: [
            hits:: hits + 1.
            hits:: hits + 2].
          hits:: hits + 10].
        ^hits
      )
      public iterating = (
        | total |
        total:: 0.
        {1. 2. 3} do: [:each |
          total:: total + each.
          total:: total * 1].
        ^total
      )
      public protecting = (
        | answer |
        answer:: [
          self boom.
          7] on: Error do: [:error |
            | partial |
            partial:: 3.
            partial + 4].
        ^answer + 1
      )
      public storing = (
        | result |
        self keep: [:value |
          result:: value + 1.
          result:: result * 2].
        ^self later: 4
      )
      public returning = (
        | block |
        block:: self made.
        ^block value: 4
      )
      public finding = (
        | found |
        found:: self find.
        ^found + 1
      )
      boom = (
        Error signal: 'boom'.
        ^1
      )
      keep: block = (
        kept:: block
      )
      later: value = (
        | answer |
        answer:: kept value: value.
        ^answer
      )
      made = (
        ^[:number |
          number + 1.
          number * 2]
      )
      find = (
        {1. 2. 3} do: [:each |
          each = 2 ifTrue: [^each * 10].
          each yourself].
        ^0
      )
      public handing = (
        | total block |
        total:: 0.
        block:: [:each |
          total:: total + (each * 5).
          total:: total * 1].
        {1. 2. 3} do: block.
        ^total
      )
      public computing = (
        | total block |
        total:: 0.
        block:: [:each |
          total:: total - each.
          total:: total * 1].
        {1. 2. 3} do: block yourself.
        ^total
      )
      public parenthesizing = (
        | total |
        total:: 0.
        {1. 2. 3} do: ([:each |
          total:: total + (each * 2).
          total:: total * 1]).
        ^total
      )
      public assigning = (
        | x y |
        x:: 1.
        y:: (x > 0 ifTrue: [
          x:: x + 20.
          x + 100] ifFalse: [0]) + 1.
        ^y
      )
      public passing = (
        | x |
        x:: 1.
        ^self twice: (x > 0
          ifTrue: [
            x:: x + 30.
            x]
          ifFalse: [0])
      )
      twice: value = (
        ^value * 2
      )
      public recursing = (
        ^self walk: 2
      )
      walk: depth = (
        | total |
        total:: 0.
        depth > 0 ifTrue: [total:: self walk: depth - 1].
        {1. 2} do: [:each |
          total:: total + (each * 3).
          total:: total + depth].
        ^total
      )
      public ensuring = (
        | total |
        total:: 0.
        [total:: total + 1] ensure: [
          total:: total + 10.
          total:: total * 2].
        ^total
      )
      public unwinding = (
        | found |
        found:: self escape.
        ^found + 1
      )
      escape = (
        [^5] ensure: [
          kept:: 6.
          kept:: 7].
        ^0
      )
      public siblings = (
        | total first second |
        total:: 0.
        first:: [
          total:: total + 1.
          total:: total + 2].
        second:: [
          first value.
          total:: total * 10].
        second value.
        ^total
      )
      public nestingStatement = (
        | c d a |
        c:: true.
        d:: true.
        a:: 0.
        c ifTrue: [d ifTrue: [
          a:: a + 1.
          a:: a + 2]] ifFalse: [a:: 9].
        ^a
      )
      public nestingAssignment = (
        | c d x |
        c:: true.
        d:: true.
        x:: 0.
        x:: c ifTrue: [d ifTrue: [
          x:: x + 4.
          x + 5]] ifFalse: [2].
        ^x
      )
      public proxying = (
        | total |
        total:: 0.
        (Proxy on: {1. 2. 3}) do: [:each |
          total:: total + (each * 7).
          total:: total * 1].
        ^total
      )
      public proxyHanding = (
        | count total block proxy |
        count:: 0.
        total:: 0.
        proxy:: Proxy on: {1. 2. 3}.
        block:: [:each |
          total:: total + (each * 12).
          total:: total * 1].
        count:: count + self one.
        proxy do: block.
        ^total + count
      )
      one = (
        ^1
      )
      public proxyStale = (
        | runner total x proxy |
        total:: 0.
        x:: 0.
        runner:: Runner new.
        proxy:: Proxy on: runner.
        x:: 1 + x.
        runner keep: [:each |
          total:: total + (each * 13).
          total:: total * 1].
        proxy run: x.
        ^total
      )
      public proxyInjecting = (
        | total |
        total:: (Proxy on: {1. 2. 3}) inject: 100 into: [:sum :each |
          sum - (each * 8).
          sum - each].
        ^total
      )
      public tailing = (
        self tail.
        ^kept
      )
      tail = (
        kept:: 0.
        {4. 5. 6} do: [:each |
          kept:: kept + (each * 9).
          kept:: kept * 1]
      )
      public truthful = (
        | truth x |
        truth:: Truth new.
        x:: 0.
        truth ifTrue: [
          x:: x + 1.
          x:: x + 2].
        ^x
      )
      public truthfulValue = (
        | truth x |
        truth:: Truth new.
        x:: 0.
        x:: (truth ifTrue: [
          x:: x + 3.
          x + 4]) + 5.
        ^x
      )
      public nestingValue = (
        | c d x |
        c:: true.
        d:: true.
        x:: 0.
        x:: c ifTrue: [(d ifTrue: [
          x:: x + 6.
          x] ifFalse: [0]) + 7] ifFalse: [2].
        ^x
      )
    ) : ()
"};

/// Where a step stopped: the top frame's name and statement, and the
/// value of the last call it reported.
type SteppedTo = ((String, String), Option<String>);

/// Sends `selector` to a `LEAVING` with a breakpoint at the line of
/// `line`, continues past its first `hits - 1` stops, removes it and takes
/// `commands`, one step after another: where each stopped, and the run's
/// answer.
fn leaving(
    lowering: Lowering,
    selector: &'static str,
    line: &str,
    hits: usize,
    commands: Vec<Command>,
) -> (Vec<SteppedTo>, i64) {
    leaving_with(lowering, true, selector, line, hits, commands)
}

/// `leaving`, the JIT on or off (`stepped_with`).
fn leaving_with(
    lowering: Lowering,
    jit: bool,
    selector: &'static str,
    line: &str,
    hits: usize,
    commands: Vec<Command>,
) -> (Vec<SteppedTo>, i64) {
    let offset = line_start(LEAVING, line);
    stepped_with(
        LEAVING,
        lowering,
        jit,
        selector,
        |remote| set_breakpoints(remote, "Leaving", vec![offset]),
        move |remote| {
            let (id, _) = resolved(&expect_breakpoints(remote, "Leaving"));
            for _ in 1..hits {
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                continue_stopped(remote);
            }
            assert_eq!(expect_breakpoint_stop(remote), [id]);
            // Only the steps stop from here.
            set_breakpoints(remote, "Leaving", Vec::new());
            assert_eq!(expect_breakpoints(remote, "Leaving"), []);
            let mut stops = Vec::new();
            for command in commands {
                let returned = step(remote, command);
                stops.push((
                    standing(remote, LEAVING),
                    returned.map(|returned| returned.label),
                ));
            }
            continue_stopped(remote);
            stops
        },
    )
}

/// A frame of `LEAVING` named `name`, standing at `statement`, with
/// `returned` reported as the value of the last call.
fn stood(name: &str, statement: &str, returned: Option<&str>) -> SteppedTo {
    (
        (String::from(name), String::from(statement)),
        returned.map(String::from),
    )
}

/// The statements `stops` stood at: what a step out of an inlined form
/// is held to, since the reference lowering runs the form's blocks as
/// frames of their own, which the optimized one lowers inline.
fn statements(stops: &[SteppedTo]) -> Vec<&str> {
    stops
        .iter()
        .map(|((_, statement), _)| statement.as_str())
        .collect()
}

/// The text of `LEAVING`'s statement that begins with `start` and ends
/// with `end`.
fn leaving_statement(start: &str, end: &str) -> String {
    let from = LEAVING.find(start).expect("the statement's start");
    let until = LEAVING[from..].find(end).expect("the statement's end") + from + end.len();
    String::from(&LEAVING[from..until])
}

ns_runtime::for_each_lowering! {
    /// A step out of a `whileTrue:`'s body leaves the whole loop, which
    /// runs its remaining rounds, and stops after it, never at the
    /// condition's next evaluation.
    fn a_step_out_of_a_loop_s_body_stops_after_the_loop(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "looping",
            "index:: index + 1.",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(stops.len(), 1);
        assert_eq!(
            stops[0].0,
            (String::from("Leaving>>looping"), String::from("^total"))
        );
        assert_eq!(answer, 15);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of a loop's body in its last round, the loop ending
    /// during the step, stops after the loop too.
    fn a_step_out_of_a_loop_s_last_round_stops_after_the_loop(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "looping",
            "index:: index + 1.",
            5,
            vec![Command::StepOut],
        );
        assert_eq!(stops.len(), 1);
        assert_eq!(
            stops[0].0,
            (String::from("Leaving>>looping"), String::from("^total"))
        );
        assert_eq!(answer, 15);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of a loop's condition, where a step over the body's
    /// last statement goes on, leaves the loop as well.
    fn a_step_out_of_a_loop_s_condition_stops_after_the_loop(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "looping",
            "total:: total + index].",
            1,
            vec![Command::StepOver, Command::StepOut],
        );
        assert_eq!(statements(&stops), ["index < 5", "^total"]);
        assert_eq!(stops[1].0 .0, "Leaving>>looping");
        assert_eq!(answer, 15);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of an `ifTrue:`'s block stops after the conditional,
    /// past the block's remaining statements.
    fn a_step_out_of_an_if_true_s_block_stops_after_the_conditional(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "conditional",
            "x:: x + 10.",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(statements(&stops), ["^x"]);
        assert_eq!(stops[0].0 .0, "Leaving>>conditional");
        assert_eq!(answer, 111);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of a block nested in another leaves one level: out of
    /// the `ifTrue:` to the loop body's next statement, then out of the
    /// loop.
    fn a_step_out_of_nested_blocks_leaves_one_level_at_a_time(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "nesting",
            "hits:: hits + 1.",
            1,
            vec![Command::StepOut, Command::StepOut],
        );
        assert_eq!(statements(&stops), ["hits:: hits + 10", "^hits"]);
        assert_eq!(stops[1].0 .0, "Leaving>>nesting");
        assert_eq!(answer, 36);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of a `do:`'s block leaves the whole `do:`, past the
    /// block's runs for the remaining elements, and stops after it in the
    /// block's home, reporting what the `do:` answered.
    fn a_step_out_of_a_do_s_block_stops_after_the_do(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "iterating",
            "total:: total + each.",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(stops.len(), 1);
        assert_eq!(
            stops[0].0,
            (String::from("Leaving>>iterating"), String::from("^total"))
        );
        assert!(stops[0].1.is_some(), "the do:'s answer is reported");
        assert_eq!(answer, 6);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of an `on:do:`'s protected block, whose error the
    /// handler takes meanwhile, stops where the `on:do:` answered.
    fn a_step_out_of_a_protected_block_stops_after_the_on_do(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "protecting",
            "self boom.",
            1,
            vec![Command::StepOut],
        );
        let statement = leaving_statement("answer:: [", "partial + 4]");
        assert_eq!(
            stops,
            [stood("Leaving>>protecting", &statement, Some("7"))]
        );
        assert_eq!(answer, 8);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of an `on:do:`'s handler block stops where the
    /// `on:do:` answered, the handler's value.
    fn a_step_out_of_a_handler_block_stops_after_the_on_do(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "protecting",
            "partial:: 3.",
            1,
            vec![Command::StepOut],
        );
        let statement = leaving_statement("answer:: [", "partial + 4]");
        assert_eq!(
            stops,
            [stood("Leaving>>protecting", &statement, Some("7"))]
        );
        assert_eq!(answer, 8);
    }
}

ns_runtime::for_each_lowering! {
    /// A block kept and run by another send than the one that took it,
    /// its home still running, steps out as any frame does: to the return
    /// point of its caller.
    fn a_step_out_of_a_block_run_from_elsewhere_stops_in_its_caller(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "storing",
            "result:: value + 1.",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(
            stops,
            [stood("Leaving>>later:", "answer:: kept value: value", Some("10"))]
        );
        assert_eq!(answer, 10);
    }
}

ns_runtime::for_each_lowering! {
    /// A block whose home returned steps out as any frame does.
    fn a_step_out_of_a_block_whose_home_returned_stops_in_its_caller(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "returning",
            "number + 1.",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(
            stops,
            [stood("Leaving>>returning", "^block value: 4", Some("8"))]
        );
        assert_eq!(answer, 8);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of a block the last statement's send of a body took
    /// stops after the send, at the return the body ends with, which no
    /// statement holds: it stands at that last statement, not at the whole
    /// body.
    fn a_step_out_of_a_block_the_last_statement_took_stands_at_that_statement(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "tailing",
            "kept:: kept + (each * 9).",
            1,
            vec![Command::StepOut],
        );
        let statement = leaving_statement("{4. 5. 6} do:", "kept * 1]");
        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0].0, (String::from("Leaving>>tail"), statement));
        assert_eq!(answer, 135);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of a block an inlined form's fallback send runs -- the
    /// optimized lowering's send of `ifTrue:` to a receiver no Boolean,
    /// the reference lowering's send of it -- leaves the form as a step
    /// over of it would: at the next statement for a form that is a whole
    /// statement, at its join for one used as a value.
    fn a_step_out_of_a_fallback_send_s_block_leaves_the_form(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "truthful",
            "x:: x + 1.",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(statements(&stops), ["^x"]);
        assert_eq!(stops[0].0 .0, "Leaving>>truthful");
        assert_eq!(answer, 3);
        let (stops, answer) = leaving(
            lowering,
            "truthfulValue",
            "x:: x + 3.",
            1,
            vec![Command::StepOut],
        );
        let statement = leaving_statement("x:: (truth ifTrue: [", "+ 5");
        assert_eq!(statements(&stops), [statement.as_str()]);
        assert_eq!(stops[0].0 .0, "Leaving>>truthfulValue");
        assert_eq!(answer, 12);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of a block a send takes leaves the send when what the
    /// send delivers to is a `doesNotUnderstand:` handler, which holds the
    /// block only in its `Message`'s arguments: the send's own arguments
    /// decide, not its callee's.
    fn a_step_out_of_a_block_a_does_not_understand_takes_stops_after_the_send(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "proxying",
            "total:: total + (each * 7).",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(stops.len(), 1);
        assert_eq!(
            stops[0].0,
            (String::from("Leaving>>proxying"), String::from("^total"))
        );
        assert_eq!(answer, 42);
    }
}

ns_runtime::for_each_lowering! {
    /// The same for a block handed on in a variable, which the send reads
    /// where it stands, not from its slot: the slot still holds what an
    /// earlier statement left there.
    fn a_step_out_of_a_block_a_does_not_understand_takes_in_a_variable_stops_after_the_send(
        lowering
    ) {
        for jit in [true, false] {
            let (stops, answer) = leaving_with(
                lowering,
                jit,
                "proxyHanding",
                "total:: total + (each * 12).",
                1,
                vec![Command::StepOut],
            );
            assert_eq!(stops.len(), 1, "the JIT on: {jit}");
            assert_eq!(
                stops[0].0,
                (String::from("Leaving>>proxyHanding"), String::from("^total + count")),
                "the JIT on: {jit}"
            );
            assert_eq!(answer, 73, "the JIT on: {jit}");
        }
    }
}

ns_runtime::for_each_lowering! {
    /// A block kept by one send and run by a later one through a
    /// `doesNotUnderstand:` handler, which took another argument, steps
    /// out as any frame does, though the later send's slot still holds the
    /// block the earlier one took.
    fn a_step_out_of_a_kept_block_a_does_not_understand_runs_stops_in_its_caller(lowering) {
        for jit in [true, false] {
            let (stops, answer) = leaving_with(
                lowering,
                jit,
                "proxyStale",
                "total:: total + (each * 13).",
                1,
                vec![Command::StepOut],
            );
            assert_eq!(stops.len(), 1, "the JIT on: {jit}");
            assert_eq!(
                stops[0].0,
                (String::from("Runner>>run:"), String::from("^kept value: value")),
                "the JIT on: {jit}"
            );
            assert_eq!(answer, 13, "the JIT on: {jit}");
        }
    }
}

ns_runtime::for_each_lowering! {
    /// The same for a block that is the send's second argument, which a
    /// `doesNotUnderstand:` handler's frame does not lie on.
    fn a_step_out_of_a_second_argument_a_does_not_understand_takes_stops_after_the_send(
        lowering
    ) {
        let (stops, answer) = leaving(
            lowering,
            "proxyInjecting",
            "sum - (each * 8).",
            1,
            vec![Command::StepOut],
        );
        let statement = leaving_statement("total:: (Proxy on:", "sum - each]");
        assert_eq!(stops.len(), 1);
        assert_eq!(
            stops[0].0,
            (String::from("Leaving>>proxyInjecting"), statement)
        );
        assert_eq!(answer, 94);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of a block a `do:` takes through a variable leaves the
    /// `do:`, as one of a literal does.
    fn a_step_out_of_a_block_handed_on_in_a_variable_stops_after_the_send(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "handing",
            "total:: total + (each * 5).",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(stops.len(), 1);
        assert_eq!(
            stops[0].0,
            (String::from("Leaving>>handing"), String::from("^total"))
        );
        assert_eq!(answer, 30);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of a block an expression computes for the `do:` leaves
    /// the `do:`.
    fn a_step_out_of_a_block_an_expression_hands_on_stops_after_the_send(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "computing",
            "total:: total - each.",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(stops.len(), 1);
        assert_eq!(
            stops[0].0,
            (String::from("Leaving>>computing"), String::from("^total"))
        );
        assert_eq!(answer, -6);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of a parenthesized block literal leaves the send that
    /// took it.
    fn a_step_out_of_a_parenthesized_block_stops_after_the_send(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "parenthesizing",
            "total:: total + (each * 2).",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(stops.len(), 1);
        assert_eq!(
            stops[0].0,
            (String::from("Leaving>>parenthesizing"), String::from("^total"))
        );
        assert_eq!(answer, 12);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of an inlined form's block mid-statement stops where the
    /// form answered, the rest of its statement still to run, as a real
    /// send's return point does.
    fn a_step_out_of_an_inlined_form_mid_statement_stops_at_its_answer(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "assigning",
            "x:: x + 20.",
            1,
            vec![Command::StepOut],
        );
        let statement = leaving_statement("y:: (x > 0 ifTrue: [", "ifFalse: [0]) + 1");
        assert_eq!(statements(&stops), [statement.as_str()]);
        assert_eq!(stops[0].0 .0, "Leaving>>assigning");
        assert_eq!(answer, 122);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of an inlined form's block that is a send's argument
    /// stops where the form answered, before the send.
    fn a_step_out_of_an_inlined_form_in_an_argument_stops_at_its_answer(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "passing",
            "x:: x + 30.",
            1,
            vec![Command::StepOut],
        );
        let statement = leaving_statement("^self twice: (x > 0", "ifFalse: [0])");
        assert_eq!(statements(&stops), [statement.as_str()]);
        assert_eq!(stops[0].0 .0, "Leaving>>passing");
        assert_eq!(answer, 62);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of the block of the innermost of three activations of a
    /// recursive method leaves that activation's `do:`, not an outer
    /// one's.
    fn a_step_out_of_a_recursive_home_s_block_stays_in_its_activation(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "recursing",
            "total:: total + (each * 3).",
            1,
            vec![Command::StepOut, Command::StepOut],
        );
        assert_eq!(stops.len(), 2);
        assert_eq!(
            stops[0].0,
            (String::from("Leaving>>walk:"), String::from("^total"))
        );
        // Out of the innermost activation, to its caller's send, which
        // the reference lowering runs in the `ifTrue:`'s block.
        assert_eq!(stops[1].0 .1, "total:: self walk: depth - 1");
        assert_eq!(stops[1].1.as_deref(), Some("9"));
        assert_eq!(answer, 33);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of an `ensure:`'s block leaves the `ensure:`.
    fn a_step_out_of_an_ensure_block_stops_after_the_ensure(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "ensuring",
            "total:: total + 10.",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(stops.len(), 1);
        assert_eq!(
            stops[0].0,
            (String::from("Leaving>>ensuring"), String::from("^total"))
        );
        assert_eq!(answer, 22);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of an `ensure:`'s block a non-local return's unwind runs
    /// stops where the return goes on: the home's caller.
    fn a_step_out_of_an_ensure_block_an_unwind_runs_stops_where_the_return_goes_on(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "unwinding",
            "kept:: 6.",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(
            stops,
            [stood("Leaving>>unwinding", "found:: self escape", Some("5"))]
        );
        assert_eq!(answer, 6);
    }
}

ns_runtime::for_each_lowering! {
    /// A closure run by a sibling closure of the same home, which the
    /// home's send did not take, steps out as any frame does: to the
    /// return point of the sibling's send, the sibling's next statement.
    fn a_step_out_of_a_closure_a_sibling_runs_stops_in_the_sibling(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "siblings",
            "total:: total + 1.",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(
            stops,
            [stood("[] in Leaving>>siblings", "total:: total * 10", Some("3"))]
        );
        assert_eq!(answer, 30);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of a form that is a whole statement of an enclosing
    /// form's block stops where a step over of that statement would: at
    /// the next statement start, never at the enclosing form's own jump
    /// after it.
    fn a_step_out_of_a_nested_form_s_statement_stops_where_a_step_over_would(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "nestingStatement",
            "a:: a + 1.",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(statements(&stops), ["^a"]);
        assert_eq!(stops[0].0 .0, "Leaving>>nestingStatement");
        assert_eq!(answer, 3);
    }
}

ns_runtime::for_each_lowering! {
    /// The same for a nested form whose enclosing form's value is
    /// assigned: the step goes on past the assignment.
    fn a_step_out_of_a_nested_form_in_an_assigned_form_stops_after_the_assignment(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "nestingAssignment",
            "x:: x + 4.",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(statements(&stops), ["^x"]);
        assert_eq!(stops[0].0 .0, "Leaving>>nestingAssignment");
        assert_eq!(answer, 9);
    }
}

ns_runtime::for_each_lowering! {
    /// A nested form that is a value inside an enclosing form's block
    /// stops at its join, mid the block's statement.
    fn a_step_out_of_a_nested_form_used_as_a_value_stops_at_its_join(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "nestingValue",
            "x:: x + 6.",
            1,
            vec![Command::StepOut],
        );
        let statement = leaving_statement("(d ifTrue: [", "ifFalse: [0]) + 7");
        assert_eq!(statements(&stops), [statement.as_str()]);
        assert_eq!(answer, 13);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of a `do:`'s block whose next run returns non-locally
    /// from the home, before the `do:` answers, stops where that return
    /// goes on: the home's caller.
    fn a_step_out_of_a_block_whose_send_a_non_local_return_leaves_stops_where_it_goes_on(lowering) {
        let (stops, answer) = leaving(
            lowering,
            "finding",
            "each yourself].",
            1,
            vec![Command::StepOut],
        );
        assert_eq!(
            stops,
            [stood("Leaving>>finding", "found:: self find", Some("20"))]
        );
        assert_eq!(answer, 21);
    }
}

/// A fallback block whose receiver's `ifTrue:` runs it, then loops for
/// good inside an `ensure:` that runs the block again, so that a step
/// into taken in the block is still pending when the program is
/// terminated, and the termination's unwind runs the block's flooded
/// statements.
const PENDING: &str = indoc! {"
    class Pending = Object new () (
      public run = (
        ^self check: Spinner new
      )
      check: condition = (
        | x |
        condition ifTrue: [
          x:: 1].
        ^x
      )
      public class Spinner = Object new () (
        public ifTrue: block = (
          | turns |
          block value.
          turns:: 0.
          [[true] whileTrue: [turns:: turns + 1]] ensure: block.
          ^nil
        )
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A termination while a step into is pending ends the step: the
    /// `ensure:` block the unwind runs, the stepping frame's flooded
    /// fallback block, makes no stop, and the run ends terminated.
    fn a_terminate_during_a_step_ends_the_step(lowering) {
        let (mut interpreter, instance) = instance_of(PENDING, lowering);
        let remote = interpreter.attach_debugger();
        set_breakpoints(&remote, "Pending", vec![line_start(PENDING, "x:: 1].")]);
        let (events, result, _) = debugged_by(
            remote,
            |remote| {
                expect_first_stop(remote, "Pending");
                // Only the step's one-shots stand in the block from here.
                set_breakpoints(remote, "Pending", Vec::new());
                expect_breakpoints(remote, "Pending");
                remote.send(Command::StepInto);
                match remote.next_event(EVENT_TIMEOUT) {
                    Some(Event::Continued) => {}
                    other => panic!("the confirmation of the step was expected, not {other:?}"),
                }
                // Long enough for the block to return into the loop.
                thread::sleep(PAUSE_DELAY);
                remote.send(Command::Terminate);
                let mut events = Vec::new();
                // A stop is continued, so that a wrong one fails the
                // assertion rather than leaving the unwind stopped.
                while let Some(event) = remote.next_event(Duration::from_millis(500)) {
                    if let Event::Stopped { .. } = event {
                        remote.send(Command::Continue);
                    }
                    events.push(event);
                }
                events
            },
            || run(&mut interpreter, &instance),
        );
        assert_no_stop(&events);
        assert_terminated(result);
    }
}

ns_runtime::for_each_lowering! {
    /// A step over begun at a pause the run's first push stopped at, its
    /// frame standing at its first statement start, which the flood
    /// patches, stops at the next statement, not where it stands. The
    /// pause asked before the run stops the sent `run` at its first
    /// statement; one asked there stops at the next safepoint, the push.
    fn a_step_over_after_a_pause_at_a_push_stops_at_the_next_statement(lowering) {
        let (mut interpreter, instance) = instance_of(STEPPING, lowering);
        let remote = interpreter.attach_debugger();
        remote.pause();
        let ((before, after), result, _) = debugged_by(
            remote,
            |remote| {
                expect_paused(remote);
                remote.pause();
                continue_stopped(remote);
                expect_paused(remote);
                let before = standing(remote, STEPPING);
                step(remote, Command::StepOver);
                let after = standing(remote, STEPPING);
                continue_stopped(remote);
                (before, after)
            },
            || run(&mut interpreter, &instance),
        );
        assert_eq!(
            before,
            (String::from("Stepping>>compute"), String::from("a:: 3"))
        );
        assert_eq!(
            after,
            (String::from("Stepping>>compute"), String::from("a:: a + 4"))
        );
        assert_eq!(answered(&mut interpreter, result), 14);
    }
}

ns_runtime::for_each_lowering! {
    /// A step into a method whose first instruction makes its indirection
    /// vector, before any statement, stops at its first statement.
    fn a_step_into_a_callee_with_an_indirection_vector_stops_at_its_first_statement(lowering) {
        let offset = line_start(STEPPING, "^self captured");
        let (stood, answer) = stepped(
            STEPPING,
            lowering,
            "vectoring",
            |remote| set_breakpoints(remote, "Stepping", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Stepping");
                step(remote, Command::StepInto);
                let stood = standing(remote, STEPPING);
                continue_stopped(remote);
                stood
            },
        );
        assert_eq!(
            stood,
            (String::from("Stepping>>captured"), String::from("b:: 5"))
        );
        assert_eq!(answer, 6);
    }
}

/// A non-local return out of an `ensure:` whose block notes it, and one
/// out of a block a `do:` runs, each answered to a caller that keeps it
/// in a temporary.
const UNWINDING: &str = indoc! {"
    class Unwinding = Object new (
      | notes ::= 0. |
    ) (
      public ensuring = (
        | x |
        x:: self home.
        ^x + notes
      )
      public finding = (
        | y |
        y:: self find.
        ^y + 1
      )
      home = (
        [^5] ensure: [
          self note].
        ^0
      )
      find = (
        {1. 2. 3} do: [:each |
          each = 2 ifTrue: [
            ^each * 10]].
        ^0
      )
      note = (
        notes:: notes + 1
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step out of an `ensure:` block the unwind of a non-local return
    /// runs, which returns to no caller's send, goes on as a step into
    /// along the unwind and stops where the program goes on: at the
    /// return point of the home's caller, which the return lands in.
    fn a_step_out_of_an_ensure_block_run_by_an_unwind_stops_where_the_program_goes_on(lowering) {
        let offset = line_start(UNWINDING, "self note].");
        let ((inside, stood, returned), answer) = stepped(
            UNWINDING,
            lowering,
            "ensuring",
            |remote| set_breakpoints(remote, "Unwinding", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Unwinding");
                let inside = standing(remote, UNWINDING);
                let returned = step(remote, Command::StepOut);
                let stood = standing(remote, UNWINDING);
                continue_stopped(remote);
                (inside, stood, returned)
            },
        );
        assert_eq!(
            inside,
            (String::from("[] in Unwinding>>home"), String::from("self note"))
        );
        assert_eq!(
            stood,
            (String::from("Unwinding>>ensuring"), String::from("x:: self home"))
        );
        assert_eq!(returned.expect("the home's answer").label, "5");
        assert_eq!(answer, 8);
    }
}

ns_runtime::for_each_lowering! {
    /// A step over a non-local return in a block a `do:` runs, the
    /// stepping frame popped by the unwind, stops at the return point of
    /// the home's caller.
    fn a_step_over_a_non_local_return_stops_in_the_home_s_caller(lowering) {
        let offset = line_start(UNWINDING, "^each * 10]].");
        let ((stood, returned), answer) = stepped(
            UNWINDING,
            lowering,
            "finding",
            |remote| set_breakpoints(remote, "Unwinding", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Unwinding");
                let returned = step(remote, Command::StepOver);
                let stood = standing(remote, UNWINDING);
                continue_stopped(remote);
                (stood, returned)
            },
        );
        assert_eq!(
            stood,
            (String::from("Unwinding>>finding"), String::from("y:: self find"))
        );
        assert_eq!(returned.expect("the home's answer").label, "20");
        assert_eq!(answer, 21);
    }
}

/// A simultaneous slot clause whose first slot hands its sibling's
/// future to a method, whose primitive `+` meets it unresolved: the
/// future's `resolve` is pushed, and once it returns the send runs again.
const FORCING: &str = indoc! {"
    class Forcing = Object new () (
      public run = (
        ^Probe new first
      )
      class Probe = Object new (
        ||
          public first = self add: second.
          public second =
            1 + 1.
        ||
      ) (
        add: value = (
          | sum |
          sum:: 1 + value.
          ^sum * 10
        )
      )
    ) : ()
"};

/// The name of the stopped top frame.
fn top_name(remote: &DebugRemote) -> String {
    stack_trace(remote)[0].name.clone()
}

/// Waits for the answer to `SetStepFilters`.
fn expect_step_filters_set(remote: &DebugRemote) {
    match remote.next_event(EVENT_TIMEOUT) {
        Some(Event::StepFiltersSet) => {}
        other => panic!("the answer to SetStepFilters was expected, not {other:?}"),
    }
}

/// What `stepped_out_of_a_future` found: the name of each frame a stop
/// stood in, the last stop's frame and statement, the value it reported,
/// and the run's answer.
struct SteppedOut {
    names: Vec<String>,
    stood: (String, String),
    returned: Option<ValueDescription>,
    answer: i64,
}

/// Steps out from the breakpoint at `line` in a future's block of
/// `source`, module `module`, until `until` is on top, the shipped filter
/// as `shipped` says.
fn stepped_out_of_a_future(
    source: &'static str,
    module: &'static str,
    line: &str,
    until: &'static str,
    lowering: Lowering,
    shipped: bool,
) -> SteppedOut {
    let offset = line_start(source, line);
    let ((names, stood, returned), answer) = stepped(
        source,
        lowering,
        "run",
        |remote| {
            remote.send(Command::SetStepFilters {
                shipped,
                classes: Vec::new(),
            });
            set_breakpoints(remote, module, vec![offset]);
        },
        move |remote| {
            expect_step_filters_set(remote);
            expect_first_stop(remote, module);
            let mut names = vec![top_name(remote)];
            // Out of the future's block and the forcing's own frames
            // until the frame that met the future is on top.
            let mut returned = None;
            while names.len() < 8 && names.last().is_none_or(|name| name != until) {
                returned = step(remote, Command::StepOut);
                names.push(top_name(remote));
            }
            let stood = standing(remote, source);
            continue_stopped(remote);
            (names, stood, returned)
        },
    );
    SteppedOut {
        names,
        stood,
        returned,
        answer,
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of a future's `resolve`, which returns to an instruction
    /// that could not take its value, goes on as a step into from the
    /// frame that met the future: the instruction runs again, its answer
    /// recorded, and the step stops at the frame's next statement. The
    /// forcing's own frames are shipped, so the step out of the future's
    /// block lands there at once.
    fn a_step_out_of_a_future_s_resolve_stops_in_its_caller(lowering) {
        let out = stepped_out_of_a_future(FORCING, "Forcing", "1 + 1.", "Probe>>add:", lowering, true);
        assert_eq!(out.names, ["[] in Probe initializer", "Probe>>add:"]);
        assert_eq!(
            out.stood,
            (String::from("Probe>>add:"), String::from("^sum * 10"))
        );
        assert_eq!(out.returned.expect("the + run again").label, "3");
        assert_eq!(out.answer, 30);
    }
}

ns_runtime::for_each_lowering! {
    /// With the shipped filter off, the same step out stops in each of
    /// the forcing's frames on its way to the frame that met the future
    /// -- out of `resolve`'s block, the receiver of its `ifCurtailed:`,
    /// past the whole `ifCurtailed:` into `resolve` (decision 17).
    fn with_the_shipped_filter_off_a_step_out_of_a_future_s_resolve_stops_in_its_frames(lowering) {
        let out = stepped_out_of_a_future(FORCING, "Forcing", "1 + 1.", "Probe>>add:", lowering, false);
        assert_eq!(
            out.names,
            [
                "[] in Probe initializer",
                "[] in FutureImplementation>>resolve",
                "FutureImplementation>>resolve",
                "Probe>>add:",
            ]
        );
        assert_eq!(
            out.stood,
            (String::from("Probe>>add:"), String::from("^sum * 10"))
        );
        assert_eq!(out.returned.expect("the + run again").label, "3");
        assert_eq!(out.answer, 30);
    }
}

/// A nested class whose superclass clause sends a method of its
/// enclosing object, built afresh for each new `Outer`.
const CLAUSED: &str = indoc! {"
    class Claused = Object new () (
      public run = (
        ^Outer new make
      )
      class Base = Object new () (
        public answer = (
          ^7
        )
      )
      class Outer = Object new () (
        public make = (
          | made |
          made:: Inner new.
          ^made answer
        )
        base = (
          ^Base
        )
        class Inner =
          self base new () ()
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step out of a superclass clause, whose class the accessor's send
    /// is handed (`Continuation::NestedClass`), stops at the accessor's
    /// return point, as a step out of a method stops at its send's: the
    /// breakpoint in the method the clause sends, a step out of it stops
    /// in the clause.
    fn a_step_out_of_a_superclass_clause_stops_at_the_accessor_s_return_point(lowering) {
        let offset = line_start(CLAUSED, "^Base");
        let ((clause, stood), answer) = stepped(
            CLAUSED,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Claused", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Claused");
                step(remote, Command::StepOut);
                let clause = top_name(remote);
                step(remote, Command::StepOut);
                let stood = standing(remote, CLAUSED);
                continue_stopped(remote);
                (clause, stood)
            },
        );
        assert_eq!(clause, "Inner superclass clause");
        assert_eq!(
            stood,
            (String::from("Outer>>make"), String::from("made:: Inner new"))
        );
        assert_eq!(answer, 7);
    }
}

/// A nested class whose superclass clause answers no class, which a
/// handler in the method that sends the accessor takes.
const REFUSED: &str = indoc! {"
    class Refused = Object new () (
      public run = (
        | made |
        made:: Outer new make.
        ^made
      )
      class Outer = Object new () (
        public make = (
          ^[Inner new] on: Error do: [:error | 9]
        )
        notAClass = (
          ^3
        )
        class Inner =
          self notAClass new () ()
      )
    ) : ()
"};

/// The name of the stopped top frame and the text of its statement, in
/// `source` or, for a frame of the kernel, in `Kernel.ns`.
fn standing_in_either(remote: &DebugRemote, source: &str) -> (String, String) {
    let frames = stack_trace(remote);
    let top = &frames[0];
    let location = top
        .location
        .clone()
        .unwrap_or_else(|| panic!("{} is located", top.name));
    let text = if location.module == "Kernel" {
        KERNEL
    } else {
        source
    };
    (top.name.clone(), location.statement.text(text).to_owned())
}

/// Where two steps out from the breakpoint in `REFUSED`'s `notAClass`
/// stopped, the shipped filter as `shipped` says: the frame the first
/// stopped in, the frame and statement the second stopped at, and the
/// run's answer.
fn stepped_out_of_a_refused_clause(
    lowering: Lowering,
    shipped: bool,
) -> (String, (String, String), i64) {
    let offset = line_start(REFUSED, "^3");
    let ((clause, stood), answer) = stepped(
        REFUSED,
        lowering,
        "run",
        |remote| {
            remote.send(Command::SetStepFilters {
                shipped,
                classes: Vec::new(),
            });
            set_breakpoints(remote, "Refused", vec![offset]);
        },
        |remote| {
            expect_step_filters_set(remote);
            expect_first_stop(remote, "Refused");
            step(remote, Command::StepOut);
            let clause = top_name(remote);
            step(remote, Command::StepOut);
            let stood = standing_in_either(remote, REFUSED);
            continue_stopped(remote);
            (clause, stood)
        },
    );
    (clause, stood, answer)
}

ns_runtime::for_each_lowering! {
    /// A step out of a superclass clause whose answer is refused as no
    /// class goes on where the error's handling takes the program, not
    /// into the `signal` the refusal raises: the step waits for the
    /// handler search, which finds the handler in `make` and reports it,
    /// and stops at the handler block's first statement, through the
    /// shipped machinery the filter passes.
    fn a_step_out_of_a_superclass_clause_whose_class_is_refused_does_not_stop_in_its_signal(lowering) {
        let (clause, stood, answer) = stepped_out_of_a_refused_clause(lowering, true);
        assert_eq!(clause, "Inner superclass clause");
        assert_eq!(
            stood,
            (String::from("[] in Outer>>make"), String::from("9"))
        );
        assert_eq!(answer, 9);
    }
}

ns_runtime::for_each_lowering! {
    /// With the shipped filter off, the same step goes on from the frame
    /// on top where the safepoint reports the handler found, the kernel's
    /// `cull:` that runs it, and stops at its first statement.
    fn with_the_shipped_filter_off_a_step_out_of_a_refused_clause_stops_in_the_handler_s_cull(lowering) {
        let (clause, stood, answer) = stepped_out_of_a_refused_clause(lowering, false);
        assert_eq!(clause, "Inner superclass clause");
        assert_eq!(
            stood,
            (
                String::from("Closure>>cull:"),
                String::from(
                    "^self numArgs = 0 ifTrue: [self value] ifFalse: [self value: argument]"
                )
            )
        );
        assert_eq!(answer, 9);
    }
}

/// A protected block whose first statement signals an `Error`, which the
/// handler block takes.
const HANDLED: &str = indoc! {"
    class Handled = Object new (
      | count ::= 0. |
    ) (
      public run = (
        [
          self fail.
          count:: 1]
            on: Error
            do: [:error |
              self recover].
        ^count
      )
      fail = (
        ^Error new signal: 'failed'
      )
      recover = (
        count:: count + 5
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step over a statement that raises an error a handler at the
    /// stepping frame takes stops at the handler block's first statement:
    /// the handler runs above the raising frame, before anything unwinds.
    fn a_step_over_a_statement_that_raises_stops_in_the_handler_block(lowering) {
        let offset = line_start(HANDLED, "self fail.");
        let (stood, answer) = stepped(
            HANDLED,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Handled", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Handled");
                step(remote, Command::StepOver);
                let stood = standing(remote, HANDLED);
                continue_stopped(remote);
                stood
            },
        );
        assert_eq!(
            stood,
            (String::from("[] in Handled>>run"), String::from("self recover"))
        );
        assert_eq!(answer, 15);
    }
}

/// A value class whose first slot's initializer is the first instruction
/// of its statement and is handed a sibling's future, which it cannot
/// store: the future is forced, and the instruction runs again.
const REEXECUTED: &str = indoc! {"
    class Reexecuted = Object <: Value () (
      public run = (
        ^Probe new pair first
      )
      class Pair first: firstValue second: secondValue = Value new (
        |
          public first = firstValue.
          public second = secondValue.
        |
      ) ()
      class Probe = Object new (
        ||
          public pair = Pair first: third second: 1.
          public third = 3.
        ||
      ) ()
    ) : ()
"};

/// `REEXECUTED` with the computation of the sibling the value's slot is
/// handed on a line of its own.
const PAIRED: &str = indoc! {"
    class Paired = Object <: Value () (
      public run = (
        ^Probe new pair first
      )
      class Pair first: firstValue second: secondValue = Value new (
        |
          public first = firstValue.
          public second = secondValue.
        |
      ) ()
      class Probe = Object new (
        ||
          public pair = Pair first: third second: 1.
          public third =
            3.
        ||
      ) ()
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step out of a future's `resolve` returning to an instruction that
    /// is no call, which no return point follows -- a value's slot
    /// initialized with the future -- stops in the frame that met it, at
    /// its next statement, once the instruction ran again.
    fn a_step_out_of_a_future_s_resolve_into_a_slot_initializer_stops_at_the_next_statement(lowering) {
        let out = stepped_out_of_a_future(PAIRED, "Paired", "3.", "Pair initializer", lowering, true);
        assert_eq!(out.names, ["[] in Probe initializer", "Pair initializer"]);
        assert_eq!(
            out.stood,
            (
                String::from("Pair initializer"),
                String::from("public second = secondValue.")
            )
        );
        assert_eq!(out.answer, 3);
    }
}

ns_runtime::for_each_lowering! {
    /// With the shipped filter off, the same step out stops in each of
    /// the forcing's frames first, out of `resolve`'s block past the
    /// whole `ifCurtailed:` that took it.
    fn with_the_shipped_filter_off_a_step_out_into_a_slot_initializer_stops_in_the_forcing_s_frames(lowering) {
        let out = stepped_out_of_a_future(PAIRED, "Paired", "3.", "Pair initializer", lowering, false);
        assert_eq!(
            out.names,
            [
                "[] in Probe initializer",
                "[] in FutureImplementation>>resolve",
                "FutureImplementation>>resolve",
                "Pair initializer",
            ]
        );
        assert_eq!(
            out.stood,
            (
                String::from("Pair initializer"),
                String::from("public second = secondValue.")
            )
        );
        assert_eq!(out.answer, 3);
    }
}

/// Continues the stopped program and answers how many times it stopped
/// again before its run ended, each stop continued.
fn stops_after_continuing(remote: &DebugRemote) -> usize {
    continue_stopped(remote);
    stops_until_the_end(remote)
}

ns_runtime::for_each_lowering! {
    /// A breakpoint at an instruction that meets an unresolved future
    /// stops once: the instruction the future's `resolve` returns to runs
    /// again past the breakpoint, and the slot is initialized once.
    fn a_breakpoint_at_a_reexecuted_index_stops_once(lowering) {
        let offset = line_start(REEXECUTED, "public first = firstValue.");
        let (again, answer) = stepped(
            REEXECUTED,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Reexecuted", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Reexecuted");
                stops_after_continuing(remote)
            },
        );
        assert_eq!(again, 0);
        assert_eq!(answer, 3);
    }
}

/// A method whose `do:` over an array sends a method of the program from
/// its block, and a statement after the `do:`.
const COLLECTING: &str = indoc! {"
    class Collecting = Object new (
      | total ::= 0. |
    ) (
      public run = (
        {1. 2. 3} do: [:each |
          self note: each].
        ^total
      )
      note: value = (
        total:: total + value
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step into a `do:` over an array lands in the block it runs, not
    /// in the shipped `Array>>do:`, which the step goes through.
    fn a_step_into_a_collection_s_do_lands_in_the_block(lowering) {
        let offset = line_start(COLLECTING, "{1. 2. 3} do: [:each |");
        let ((stood, each), answer) = stepped(
            COLLECTING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Collecting", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Collecting");
                step(remote, Command::StepInto);
                let stood = standing(remote, COLLECTING);
                let each = top_variable(remote, "each");
                continue_stopped(remote);
                (stood, each)
            },
        );
        assert_eq!(
            stood,
            (
                String::from("[] in Collecting>>run"),
                String::from("self note: each")
            )
        );
        assert_eq!(each, "1");
        assert_eq!(answer, 18);
    }
}

ns_runtime::for_each_lowering! {
    /// A step over at the last statement of a block a `do:` runs lands in
    /// the block's next activation, not at its return point inside the
    /// shipped `Array>>do:`.
    fn a_step_over_at_the_block_s_last_statement_lands_in_its_next_activation(lowering) {
        let offset = line_start(COLLECTING, "self note: each].");
        let ((stood, each), answer) = stepped(
            COLLECTING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Collecting", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Collecting");
                // Only the step stops from here.
                set_breakpoints(remote, "Collecting", Vec::new());
                expect_breakpoints(remote, "Collecting");
                step(remote, Command::StepOver);
                let stood = standing(remote, COLLECTING);
                let each = top_variable(remote, "each");
                continue_stopped(remote);
                (stood, each)
            },
        );
        assert_eq!(
            stood,
            (
                String::from("[] in Collecting>>run"),
                String::from("self note: each")
            )
        );
        assert_eq!(each, "2");
        assert_eq!(answer, 18);
    }
}

/// A `do:` over two thousand elements, whose block notes the 1,500th:
/// long enough for the suspend stress mode to suspend `run` below the
/// shipped `Array>>do:` before then.
#[cfg(debug_assertions)]
const SUSPENDING: &str = indoc! {"
    class Suspending = Object new (
      | count ::= 0. |
    ) (
      public run = (
        (Array new: 2000) do: [:each |
          count:: count + 1.
          count = 1500 ifTrue: [
            self mark]].
        ^count
      )
      mark = (
        ^nil
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// Under the suspend stress mode `run`, the sender of the shipped
    /// `do:`, is suspended into its frame while the block runs: a step
    /// over at the block's last statement stops at the return point in
    /// the filtered `do:`, goes on as a step into from the first
    /// unfiltered frame below it -- `run`, suspended, which the step
    /// holds as such -- and stops in the block's next activation, which
    /// `do:` pushes for that frame, as it does with nothing suspended.
    #[cfg(debug_assertions)]
    fn a_step_through_suspended_frames_stops_in_the_next_activation(lowering) {
        let (mut interpreter, instance) = instance_of(SUSPENDING, lowering);
        interpreter.set_stress_suspend(true);
        let offset = line_start(SUSPENDING, "self mark]].");
        let remote = interpreter.attach_debugger();
        set_breakpoints(&remote, "Suspending", vec![offset]);
        let (stood, result, _) = debugged_by(
            remote,
            |remote| {
                let (id, _) = resolved(&expect_breakpoints(remote, "Suspending"));
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                step(remote, Command::StepOver);
                let stood = standing(remote, SUSPENDING);
                continue_stopped(remote);
                stood
            },
            || run(&mut interpreter, &instance),
        );
        assert_eq!(answered(&mut interpreter, result), 2000);
        assert!(
            interpreter.stress_suspensions() > 0,
            "the run's frames were suspended"
        );
        assert_eq!(
            interpreter.steps_from_suspended_frames(),
            1,
            "the step went on from `run`, suspended"
        );
        assert_eq!(
            stood,
            (
                String::from("[] in Suspending>>run"),
                String::from("count:: count + 1")
            )
        );
    }
}

ns_runtime::for_each_lowering! {
    /// A step over at the last statement of the block's last activation
    /// lands in the method that sent `do:`, at the statement after it.
    fn a_step_over_after_the_last_element_lands_in_the_caller(lowering) {
        let offset = line_start(COLLECTING, "self note: each].");
        let (stood, answer) = stepped(
            COLLECTING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Collecting", vec![offset]),
            |remote| {
                let (id, _) = resolved(&expect_breakpoints(remote, "Collecting"));
                for _ in 0..2 {
                    assert_eq!(expect_breakpoint_stop(remote), [id]);
                    continue_stopped(remote);
                }
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                assert_eq!(top_variable(remote, "each"), "3");
                set_breakpoints(remote, "Collecting", Vec::new());
                expect_breakpoints(remote, "Collecting");
                step(remote, Command::StepOver);
                let stood = standing(remote, COLLECTING);
                continue_stopped(remote);
                stood
            },
        );
        assert_eq!(
            stood,
            (String::from("Collecting>>run"), String::from("^total"))
        );
        assert_eq!(answer, 18);
    }
}

ns_runtime::for_each_lowering! {
    /// With the shipped filter off, a step into the `do:` stops in the
    /// shipped code it runs.
    fn with_the_shipped_filter_off_a_step_into_lands_in_do(lowering) {
        let offset = line_start(COLLECTING, "{1. 2. 3} do: [:each |");
        let (module, answer) = stepped(
            COLLECTING,
            lowering,
            "run",
            |remote| {
                remote.send(Command::SetStepFilters {
                    shipped: false,
                    classes: Vec::new(),
                });
                set_breakpoints(remote, "Collecting", vec![offset]);
            },
            |remote| {
                expect_step_filters_set(remote);
                expect_first_stop(remote, "Collecting");
                step(remote, Command::StepInto);
                let top = stack_trace(remote).remove(0);
                continue_stopped(remote);
                top.location.map(|location| location.module)
            },
        );
        assert_eq!(module.as_deref(), Some("Kernel"));
        assert_eq!(answer, 18);
    }
}

/// A method that hands a block of its own to a nested class of the
/// program, which runs it.
const HELPING: &str = indoc! {"
    class Helping = Object new (
      | total ::= 0. |
    ) (
      public run = (
        Helper new run: [
          self note: 4].
        ^total
      )
      note: value = (
        total:: total + value
      )
      class Helper = Object new () (
        public run: action = (
          ^action value
        )
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A program class the filter names is stepped through as shipped
    /// code is: a step into the send to `Helper` lands in the caller's
    /// block that `Helper` runs.
    fn a_filtered_program_class_is_stepped_over(lowering) {
        let offset = line_start(HELPING, "Helper new run: [");
        let (stood, answer) = stepped(
            HELPING,
            lowering,
            "run",
            |remote| {
                remote.send(Command::SetStepFilters {
                    shipped: true,
                    classes: vec![ClassName {
                        module: String::from("Helping"),
                        path: vec![String::from("Helper")],
                    }],
                });
                set_breakpoints(remote, "Helping", vec![offset]);
            },
            |remote| {
                expect_step_filters_set(remote);
                expect_first_stop(remote, "Helping");
                step(remote, Command::StepInto);
                let stood = standing(remote, HELPING);
                continue_stopped(remote);
                stood
            },
        );
        assert_eq!(
            stood,
            (String::from("[] in Helping>>run"), String::from("self note: 4"))
        );
        assert_eq!(answer, 12);
    }
}

ns_runtime::for_each_lowering! {
    /// Without the filter naming it, the same step into stops in
    /// `Helper`'s code.
    fn an_unfiltered_program_class_is_stepped_into(lowering) {
        let offset = line_start(HELPING, "Helper new run: [");
        let (names, answer) = stepped(
            HELPING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Helping", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Helping");
                step(remote, Command::StepInto);
                let names: Vec<String> = stack_trace(remote)
                    .into_iter()
                    .map(|frame| frame.name)
                    .collect();
                continue_stopped(remote);
                names
            },
        );
        assert!(
            names[0].starts_with("Helper"),
            "the step stops in Helper's code, not in {names:?}"
        );
        assert_eq!(answer, 12);
    }
}

/// `HELPING`'s send twice over.
const HELPING_TWICE: &str = indoc! {"
    class HelpingTwice = Object new (
      | total ::= 0. |
    ) (
      public run = (
        Helper new run: [
          self note: 4].
        Helper new run: [
          self note: 5].
        ^total
      )
      note: value = (
        total:: total + value
      )
      class Helper = Object new () (
        public run: action = (
          ^action value
        )
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// The step filters' decisions, kept by body, are forgotten when the
    /// filters change: a step into lands in the block past the filtered
    /// `Helper`, and once the filter is dropped a step over from the
    /// block's end stops in `Helper`'s code, its return point.
    fn a_changed_step_filter_decides_afresh(lowering) {
        let offset = line_start(HELPING_TWICE, "Helper new run: [");
        let ((first, second), answer) = stepped(
            HELPING_TWICE,
            lowering,
            "run",
            |remote| {
                remote.send(Command::SetStepFilters {
                    shipped: true,
                    classes: vec![ClassName {
                        module: String::from("HelpingTwice"),
                        path: vec![String::from("Helper")],
                    }],
                });
                set_breakpoints(remote, "HelpingTwice", vec![offset]);
            },
            |remote| {
                expect_step_filters_set(remote);
                expect_first_stop(remote, "HelpingTwice");
                step(remote, Command::StepInto);
                let first = standing(remote, HELPING_TWICE);
                remote.send(Command::SetStepFilters {
                    shipped: true,
                    classes: Vec::new(),
                });
                expect_step_filters_set(remote);
                step(remote, Command::StepOver);
                let second = top_name(remote);
                continue_stopped(remote);
                (first, second)
            },
        );
        assert_eq!(
            first,
            (String::from("[] in HelpingTwice>>run"), String::from("self note: 4"))
        );
        assert!(
            second.starts_with("Helper"),
            "the step stops in Helper's code, not in {second}"
        );
        assert_eq!(answer, 27, "the warming runs added to the total");
    }
}

ns_runtime::for_each_lowering! {
    /// A breakpoint the user set in filtered code stops, and a step
    /// begun there steps in it as in any other frame.
    fn a_breakpoint_in_filtered_code_still_stops_and_steps_there(lowering) {
        let offset = line_start(
            KERNEL,
            "        action value: (self at: index).\n        index:: index + 1.",
        );
        let ((first, stood), answer) = stepped(
            COLLECTING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Kernel", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Kernel");
                let first = standing(remote, KERNEL);
                set_breakpoints(remote, "Kernel", Vec::new());
                expect_breakpoints(remote, "Kernel");
                step(remote, Command::StepOver);
                let stood = standing(remote, KERNEL);
                continue_stopped(remote);
                (first, stood)
            },
        );
        // The reference lowering runs the loop's body as a block.
        let frame = match lowering {
            Lowering::Optimized => "Array>>do:",
            Lowering::Reference => "[] in Array>>do:",
        };
        assert_eq!(
            first,
            (
                String::from(frame),
                String::from("action value: (self at: index)")
            )
        );
        assert_eq!(
            stood,
            (String::from(frame), String::from("index:: index + 1"))
        );
        assert_eq!(answer, 18);
    }
}

/// A method whose first statement sends `ifTrue:` with a literal block,
/// which the optimized lowering inlines and the reference lowering sends
/// to the shipped `True>>ifTrue:` and `False>>ifTrue:`, whose fast paths
/// answer it without an activation.
const BRANCHING: &str = indoc! {"
    class Branching = Object new () (
      public run = (
        ^(self check: 1) + (self check: 5)
      )
      check: number = (
        number <= 1 ifTrue: [
          ^1].
        ^number * 2
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step into at a send of `ifTrue:` with a literal block makes the
    /// same stops under both lowerings: in the block for a true
    /// condition, at the next statement for a false one. It pins that the
    /// two lowerings stop alike; nothing shipped is pushed, since the
    /// shipped `ifTrue:` the reference lowering sends has a fast path.
    fn a_step_into_an_inlinable_send_stops_alike_under_both_lowerings(lowering) {
        let offset = line_start(BRANCHING, "number <= 1 ifTrue: [");
        let (stops, answer) = stepped(
            BRANCHING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Branching", vec![offset]),
            |remote| {
                let (id, _) = resolved(&expect_breakpoints(remote, "Branching"));
                let mut stops = Vec::new();
                for _ in 0..2 {
                    assert_eq!(expect_breakpoint_stop(remote), [id]);
                    step(remote, Command::StepInto);
                    let top = stack_trace(remote).remove(0);
                    let location = top.location.expect("the stop is located");
                    stops.push((
                        location.module,
                        location.statement.text(BRANCHING).to_owned(),
                    ));
                    continue_stopped(remote);
                }
                stops
            },
        );
        assert_eq!(
            stops,
            [
                (String::from("Branching"), String::from("^1")),
                (String::from("Branching"), String::from("^number * 2")),
            ]
        );
        assert_eq!(answer, 11);
    }
}

/// A method that folds an array with `inject:into:`, its block a statement
/// of its own line: the shipped `inject:into:` sends `do:`, which runs
/// `inject:into:`'s own block, which runs the program's.
const FOLDING: &str = indoc! {"
    class Folding = Object new () (
      public run = (
        | sum |
        sum:: {1. 2. 3} inject: 0 into: [:total :each |
          total + each].
        ^sum
      )
    ) : ()
"};

/// The line of `Array>>do:` that runs its action, in the kernel's text.
fn do_s_action_line() -> u32 {
    line_start(
        KERNEL,
        "        action value: (self at: index).\n        index:: index + 1.",
    )
}

ns_runtime::for_each_lowering! {
    /// A step out begun in a filtered frame, stopped there by a
    /// breakpoint, lands in its unfiltered caller: `Array>>do:`'s return
    /// point in the method that sent it.
    fn a_step_out_of_a_filtered_frame_lands_in_its_unfiltered_caller(lowering) {
        let ((stood, returned), answer) = stepped(
            COLLECTING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Kernel", vec![do_s_action_line()]),
            |remote| {
                expect_first_stop(remote, "Kernel");
                set_breakpoints(remote, "Kernel", Vec::new());
                expect_breakpoints(remote, "Kernel");
                let returned = step(remote, Command::StepOut);
                let stood = standing(remote, COLLECTING);
                continue_stopped(remote);
                (stood, returned)
            },
        );
        // The return point is the index after the send, the first of the
        // statement after it.
        assert_eq!(
            stood,
            (String::from("Collecting>>run"), String::from("^total"))
        );
        assert_eq!(returned.expect("do:'s answer").class_name, "Array");
        assert_eq!(answer, 18);
    }
}

ns_runtime::for_each_lowering! {
    /// A step out of `Array>>do:` whose caller, `inject:into:`, is
    /// filtered too goes on through it, as a step into from the first
    /// unfiltered frame, the method that sent `inject:into:`, and lands at
    /// its next statement start, reporting what `inject:into:` answered.
    fn a_step_out_through_a_filtered_caller_lands_in_the_first_unfiltered_frame(lowering) {
        let ((stood, returned), answer) = stepped(
            FOLDING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Kernel", vec![do_s_action_line()]),
            |remote| {
                expect_first_stop(remote, "Kernel");
                set_breakpoints(remote, "Kernel", Vec::new());
                expect_breakpoints(remote, "Kernel");
                let returned = step(remote, Command::StepOut);
                let stood = standing(remote, FOLDING);
                continue_stopped(remote);
                (stood, returned)
            },
        );
        assert_eq!(
            stood,
            (String::from("Folding>>run"), String::from("^sum"))
        );
        assert_eq!(returned.expect("inject:into:'s answer").label, "6");
        assert_eq!(answer, 6);
    }
}

ns_runtime::for_each_lowering! {
    /// A step into a send whose callee pushes a chain of filtered frames
    /// -- `inject:into:`, `do:`, `inject:into:`'s block -- lands in the
    /// program's block at the end of it, not before.
    fn a_step_into_through_a_filtered_push_chain_lands_in_the_program_s_block(lowering) {
        let offset = line_start(FOLDING, "sum:: {1. 2. 3} inject: 0 into:");
        let ((stood, names), answer) = stepped(
            FOLDING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Folding", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Folding");
                step(remote, Command::StepInto);
                let stood = standing(remote, FOLDING);
                let names: Vec<String> = stack_trace(remote)
                    .into_iter()
                    .map(|frame| frame.name)
                    .collect();
                continue_stopped(remote);
                (stood, names)
            },
        );
        assert_eq!(
            stood,
            (String::from("[] in Folding>>run"), String::from("total + each"))
        );
        assert!(
            names.iter().any(|name| name.ends_with("inject:into:")),
            "the block runs under inject:into:: {names:?}"
        );
        assert_eq!(answer, 6);
    }
}

/// How many of `events` are stops.
fn stop_count(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, Event::Stopped { .. }))
        .count()
}

ns_runtime::for_each_lowering! {
    /// Sending the breakpoints a module already has again changes no
    /// slot: no body loses its machine code, and the breakpoint still
    /// stops.
    fn resending_unchanged_breakpoints_frees_no_machine_code(lowering) {
        let (mut interpreter, instance, _) = compiled(COMPILED_LOOP, lowering);
        assert_eq!(send_answered(&mut interpreter, &instance, "warm"), 2000);
        let offset = line_start(COMPILED_LOOP, "self bump].");
        let remote = interpreter.attach_debugger();
        set_breakpoints(&remote, "CompiledLoop", vec![offset]);
        let (events, result, remote) = events_during(remote, || run(&mut interpreter, &instance));
        assert_eq!(answered(&mut interpreter, result), 2003);
        assert_eq!(stop_count(&events), 3, "{events:?}");
        let before = interpreter.jit_statistics();
        set_breakpoints(&remote, "CompiledLoop", vec![offset]);
        let (events, result, _) = events_during(remote, || run(&mut interpreter, &instance));
        assert_eq!(answered(&mut interpreter, result), 2006);
        assert_eq!(stop_count(&events), 3, "{events:?}");
        let after = interpreter.jit_statistics();
        if runs_in_machine_code(lowering) {
            assert_eq!(
                after.freed_bodies, before.freed_bodies,
                "the unchanged breakpoint freed no code: {before:?} {after:?}"
            );
        }
    }
}

/// `check:with:` sends `ifTrue:` to `true` with an object that is no
/// block, which activates `True>>ifTrue:` though it has a fast path --
/// the fast path takes only a block. That object's `value` sends
/// `ifTrue:` again and again, now with a block, which the fast path runs
/// without an activation, until a cap no passing run reaches.
const FORWARDING: &str = indoc! {"
    class Forwarding = Object new () (
      public run = (
        ^self check: true with: Action new
      )
      public check: condition with: action = (
        ^condition ifTrue: action
      )
      public class Action = Object new () (
        public value = (
          | index ::= 0. |
          [index < 10000000] whileTrue: [
            check: true with: [index:: index + 1]].
          ^index
        )
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step out of a callee whose caller runs the body of a method with
    /// a fast path patches that body with the step's one-shot first; a
    /// line breakpoint set into the same body while the step runs still
    /// switches the fast path off, so that the next send of the method
    /// with a block activates it and stops there -- the callee's loop
    /// sends it until then, so that the stop does not depend on when the
    /// breakpoint is served; removed, it switches the fast path back on.
    fn a_line_breakpoint_set_after_a_step_s_one_shot_switches_the_fast_path_off(lowering) {
        let (mut interpreter, class) = load(FORWARDING, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let quick = Some(ns_runtime::mixin::MethodFastPath::ValueOfParameter(0));
        assert_eq!(if_true_fast_path(&mut interpreter), quick);
        let remote = interpreter.attach_debugger();
        remote.send(Command::SetStepFilters {
            shipped: false,
            classes: Vec::new(),
        });
        set_breakpoints(
            &remote,
            "Forwarding",
            vec![line_start(FORWARDING, "[index < 10000000] whileTrue: [")],
        );
        let if_true = line_start(KERNEL, "public ifTrue: trueBlock = (");
        let (stopped_in, result, _) = debugged_by(
            remote,
            move |remote| {
                expect_step_filters_set(remote);
                expect_first_stop(remote, "Forwarding");
                set_breakpoints(remote, "Forwarding", Vec::new());
                expect_breakpoints(remote, "Forwarding");
                remote.send(Command::StepOut);
                // Served while the step runs, its one-shot already in
                // `ifTrue:`'s body: the loop runs until the breakpoint
                // stops it.
                set_breakpoints(remote, "Kernel", vec![if_true]);
                match remote.next_event(EVENT_TIMEOUT) {
                    Some(Event::Continued) => {}
                    other => panic!("the step's confirmation was expected, not {other:?}"),
                }
                let (id, _) = resolved(&expect_breakpoints(remote, "Kernel"));
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                let stopped_in: Vec<String> = stack_trace(remote)
                    .into_iter()
                    .take(2)
                    .map(|frame| frame.name)
                    .collect();
                set_breakpoints(remote, "Kernel", Vec::new());
                expect_breakpoints(remote, "Kernel");
                remote.send(Command::Terminate);
                stopped_in
            },
            || run(&mut interpreter, &instance),
        );
        assert_eq!(stopped_in, ["True>>ifTrue:", "Forwarding>>check:with:"]);
        assert_terminated(result);
        assert_eq!(if_true_fast_path(&mut interpreter), quick);
    }
}

/// A method breakpoint on the method `selector` of the class `path` names
/// in `module`.
fn method_breakpoint(
    module: &str,
    path: &[&str],
    class_side: bool,
    selector: &str,
) -> MethodBreakpoint {
    MethodBreakpoint {
        class: ClassName {
            module: String::from(module),
            path: path.iter().map(|name| String::from(*name)).collect(),
        },
        class_side,
        selector: String::from(selector),
        hit_condition: None,
    }
}

/// Sets the method breakpoints to `breakpoints`.
fn set_method_breakpoints(remote: &DebugRemote, breakpoints: Vec<MethodBreakpoint>) {
    remote.send(Command::SetMethodBreakpoints(breakpoints));
}

/// Waits for the method breakpoints' states: the answer to
/// `SetMethodBreakpoints`, or the report of a resolution.
fn expect_method_breakpoints(remote: &DebugRemote) -> Vec<MethodBreakpointState> {
    match remote.next_event(EVENT_TIMEOUT) {
        Some(Event::MethodBreakpoints(states)) => states,
        other => panic!("the method breakpoints were expected, not {other:?}"),
    }
}

/// The one method breakpoint of `states`, verified as `verified` says:
/// its id.
fn only_method_breakpoint(states: &[MethodBreakpointState], verified: bool) -> BreakpointId {
    match states {
        [only] if only.verified == verified => only.id,
        other => panic!("one method breakpoint, verified {verified}, was expected, not {other:?}"),
    }
}

ns_runtime::for_each_lowering! {
    /// A method breakpoint stands in every built module of its module's
    /// name, as a line breakpoint does: a run of an instance of either
    /// module stops at the method's entry and at its exit.
    fn a_method_breakpoint_stands_in_every_module_of_its_name(lowering) {
        let module = ns_syntax::parse(THREE_STATEMENTS).expect("parse");
        let resolved = ns_runtime::resolve(
            module,
            THREE_STATEMENTS.into(),
            ns_runtime::SourceOrigin::File("test.ns".into()),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let classes = [
            interpreter.load_module(&resolved),
            interpreter.load_module(&resolved),
        ];
        let instances = classes.map(|class| expect_ok(interpreter.instantiate(&class, &[]), "instantiate"));
        let remote = interpreter.attach_debugger();
        set_method_breakpoints(&remote, vec![method_breakpoint("Lines", &[], false, "run")]);
        let mut stops = Vec::new();
        let mut remote = Some(remote);
        for (index, instance) in instances.iter().enumerate() {
            let (count, result, back) = debugged_by(
                remote.take().expect("the remote"),
                move |remote| {
                    if index == 0 {
                        only_method_breakpoint(&expect_method_breakpoints(remote), true);
                    }
                    stops_until_the_end(remote)
                },
                || run(&mut interpreter, instance),
            );
            expect_ok(result, "run");
            stops.push(count);
            remote = Some(back);
        }
        assert_eq!(stops, [2, 2]);
    }
}

/// Waits for a stop and answers its reason.
fn expect_stop(remote: &DebugRemote) -> StopReason {
    match remote.next_event(EVENT_TIMEOUT) {
        Some(Event::Stopped { reason, .. }) => reason,
        other => panic!("a stop was expected, not {other:?}"),
    }
}

/// The text of the stopped top frame's statement in `source`.
fn top_statement(remote: &DebugRemote, source: &str) -> String {
    standing(remote, source).1
}

/// How many times the running program stops before its run ends, each
/// stop continued.
fn stops_until_the_end(remote: &DebugRemote) -> usize {
    events_until_the_end(remote)
        .iter()
        .filter(|event| matches!(event, Event::Stopped { .. }))
        .count()
}

/// Every event the running program reports until its run ends, each stop
/// continued; read once more after the end, so that nothing the run sent
/// last is missed.
fn events_until_the_end(remote: &DebugRemote) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        let ended = run_finished();
        match remote.next_event(Duration::from_millis(10)) {
            Some(event) => {
                if let Event::Stopped { .. } = event {
                    remote.send(Command::Continue);
                }
                events.push(event);
            }
            None if ended => return events,
            None => {}
        }
    }
}

ns_runtime::for_each_lowering! {
    /// Run to cursor, from a breakpoint, stops at the position named, a
    /// later statement of the same method, as a step's stop; it holds for
    /// one stop only, so that the next pass through the position runs on.
    fn run_to_cursor_stops_at_the_position_once(lowering) {
        let breakpoint = line_start(LOOPING, "passes:: passes + 1.");
        let cursor = line_start(LOOPING, "count:: count + index].");
        let ((statement, again), answer) = stepped(
            LOOPING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Looping", vec![breakpoint]),
            move |remote| {
                expect_first_stop(remote, "Looping");
                set_breakpoints(remote, "Looping", Vec::new());
                expect_breakpoints(remote, "Looping");
                remote.send(Command::RunToPosition {
                    module: String::from("Looping"),
                    offset: cursor,
                });
                match remote.next_event(EVENT_TIMEOUT) {
                    Some(Event::Continued) => {}
                    other => panic!("the run to cursor's confirmation was expected, not {other:?}"),
                }
                match remote.next_event(EVENT_TIMEOUT) {
                    Some(Event::Stopped {
                        reason: StopReason::Step,
                        returned: None,
                    }) => {}
                    other => panic!("the run to cursor's stop was expected, not {other:?}"),
                }
                let statement = top_statement(remote, LOOPING);
                (statement, stops_after_continuing(remote))
            },
        );
        assert_eq!(statement, "count:: count + index");
        assert_eq!(again, 0, "the run to cursor stops once");
        assert_eq!(answer, 189);
    }
}

ns_runtime::for_each_lowering! {
    /// Run to cursor at a position no statement starts at or after, or
    /// in a module not built, is refused with a reason, and the program
    /// stays stopped.
    fn run_to_an_unresolvable_position_is_refused(lowering) {
        let breakpoint = line_start(LOOPING, "passes:: passes + 1.");
        let past = u32::try_from(LOOPING.len()).expect("a test's source is small");
        let (refusals, answer) = stepped(
            LOOPING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Looping", vec![breakpoint]),
            move |remote| {
                expect_first_stop(remote, "Looping");
                set_breakpoints(remote, "Looping", Vec::new());
                expect_breakpoints(remote, "Looping");
                let mut refusals = Vec::new();
                for module in ["Looping", "Nowhere"] {
                    let event = request(
                        remote,
                        Command::RunToPosition {
                            module: String::from(module),
                            offset: past,
                        },
                    );
                    match event {
                        Event::Refused { command, reason } => refusals.push((command, reason)),
                        other => panic!("a refusal was expected, not {other:?}"),
                    }
                }
                // Still stopped: the stack is served.
                assert_eq!(top_statement(remote, LOOPING), "passes:: passes + 1");
                continue_stopped(remote);
                refusals
            },
        );
        assert_eq!(refusals.len(), 2);
        for (command, reason) in &refusals {
            assert_eq!(command, "RunToPosition");
            assert!(!reason.is_empty());
        }
        assert_ne!(refusals[0].1, refusals[1].1, "each says why");
        assert_eq!(answer, 189);
    }
}

/// A method with two returns: its own `^number`, and `^number * 2` in a
/// block the optimized lowering inlines and the reference lowering runs.
const RETURNING: &str = indoc! {"
    class Returning = Object new () (
      public run = (
        ^(self pick: 1) + (self pick: 5)
      )
      pick: number = (
        number > 3 ifTrue: [
          ^number * 2].
        ^number
      )
    ) : ()
"};

/// Which end of a method a method breakpoint's stop for `id` stands at,
/// for a stop that names no other breakpoint.
fn method_end(reason: StopReason, id: BreakpointId) -> &'static str {
    match reason {
        StopReason::MethodEntry {
            id: stopped,
            others,
        } if stopped == id && others.is_empty() => "entry",
        StopReason::MethodExit {
            id: stopped,
            others,
        } if stopped == id && others.is_empty() => "exit",
        other => panic!("a stop of method breakpoint {id:?} alone was expected, not {other:?}"),
    }
}

ns_runtime::for_each_lowering! {
    /// A method breakpoint stops at the method's entry, at its first
    /// statement, and at the return it takes, whichever of its returns
    /// that is, and nowhere else.
    fn a_method_breakpoint_stops_at_entry_and_at_each_return(lowering) {
        let ((stops, again), answer) = stepped(
            RETURNING,
            lowering,
            "run",
            |remote| {
                set_method_breakpoints(
                    remote,
                    vec![method_breakpoint("Returning", &[], false, "pick:")],
                );
            },
            |remote| {
                let id = only_method_breakpoint(&expect_method_breakpoints(remote), true);
                let mut stops = Vec::new();
                for _ in 0..4 {
                    let end = method_end(expect_stop(remote), id);
                    let statement = top_statement(remote, RETURNING);
                    let number = top_variable(remote, "number");
                    stops.push((end, statement, number));
                    continue_stopped(remote);
                }
                (stops, stops_until_the_end(remote))
            },
        );
        let entry = "number > 3 ifTrue: [\n      ^number * 2]";
        assert_eq!(
            stops,
            [
                ("entry", String::from(entry), String::from("1")),
                ("exit", String::from("^number"), String::from("1")),
                ("entry", String::from(entry), String::from("5")),
                ("exit", String::from("^number * 2"), String::from("5")),
            ]
        );
        assert_eq!(again, 0);
        assert_eq!(answer, 11);
    }
}

/// A method a block of its own returns from non-locally, which the host
/// sends, and a second method for the next run.
const CARRYING: &str = indoc! {"
    class Carrying = Object new () (
      public first = (
        self each: [:value |
          ^value * 10].
        ^0
      )
      each: action = (
        action value: 2.
        ^nil
      )
      public second = (
        | x |
        x:: 3.
        ^x + 1
      )
    ) : ()
"};

/// Runs `CARRYING`'s `first` under `lowering`, a method breakpoint on it
/// continued past its entry, so that the unwind that arrives at it leaves
/// its exit to a step that leaves the run, then `second`, a pause asked
/// between the two runs when `paused`: what the second run's first stop
/// was, against the breakpoint's id, and where it stood.
fn carried_exit(lowering: Lowering, paused: bool) -> (&'static str, (String, String)) {
    let (mut interpreter, instance) = instance_of(CARRYING, lowering);
    let remote = interpreter.attach_debugger();
    set_method_breakpoints(
        &remote,
        vec![method_breakpoint("Carrying", &[], false, "first")],
    );
    let agent = |remote: &DebugRemote| {
        let id = only_method_breakpoint(&expect_method_breakpoints(remote), true);
        assert_eq!(method_end(expect_stop(remote), id), "entry");
        continue_stopped(remote);
        id
    };
    let (id, result, remote) = debugged_by(remote, agent, || {
        interpreter.send(&instance, Symbol::intern("first"), &[])
    });
    assert_eq!(answered(&mut interpreter, result), 20);
    if paused {
        remote.pause();
    }
    let agent = move |remote: &DebugRemote| {
        let end = method_end(expect_stop(remote), id);
        let stood = standing(remote, CARRYING);
        continue_stopped(remote);
        (end, stood)
    };
    let (stop, result, _) = debugged_by(remote, agent, || {
        interpreter.send(&instance, Symbol::intern("second"), &[])
    });
    assert_eq!(answered(&mut interpreter, result), 4);
    stop
}

ns_runtime::for_each_lowering! {
    /// The exit of a method a run's last unwind left, which the run's end
    /// carried to the next run, is reported by that run's first stop --
    /// the carried step's, or a pause's asked between the runs, which
    /// stops there instead.
    fn a_pause_between_runs_reports_the_exits_the_last_run_carried(lowering) {
        for paused in [false, true] {
            let (end, stood) = carried_exit(lowering, paused);
            assert_eq!(end, "exit", "paused: {paused}");
            assert_eq!(
                stood,
                (String::from("Carrying>>second"), String::from("x:: 3")),
                "paused: {paused}"
            );
        }
    }
}

/// A method left by the non-local return of a block it was passed, whose
/// home is its sender.
const UNWOUND: &str = indoc! {"
    class Unwound = Object new () (
      public run = (
        | found |
        found:: self find.
        ^found + 1
      )
      find = (
        self each: [:value |
          ^value * 10].
        ^0
      )
      each: action = (
        action value: 2.
        ^nil
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A method left by an unwind -- a non-local return of a block it was
    /// passed -- reports its exit where the program goes on: at the return
    /// point of the block's home's caller.
    fn a_method_breakpoint_reports_an_exit_by_unwind(lowering) {
        let ((ends, stood, again), answer) = stepped(
            UNWOUND,
            lowering,
            "run",
            |remote| {
                set_method_breakpoints(
                    remote,
                    vec![method_breakpoint("Unwound", &[], false, "each:")],
                );
            },
            |remote| {
                let id = only_method_breakpoint(&expect_method_breakpoints(remote), true);
                let entry = method_end(expect_stop(remote), id);
                continue_stopped(remote);
                let exit = method_end(expect_stop(remote), id);
                let stood = standing(remote, UNWOUND);
                continue_stopped(remote);
                ((entry, exit), stood, stops_until_the_end(remote))
            },
        );
        assert_eq!(ends, ("entry", "exit"));
        assert_eq!(
            stood,
            (String::from("Unwound>>run"), String::from("found:: self find"))
        );
        assert_eq!(again, 0);
        assert_eq!(answer, 21);
    }
}

ns_runtime::for_each_lowering! {
    /// A method its own block returns from non-locally -- a block it
    /// passed on, which the compiler did not lower inline -- reports its
    /// exit as the unwind arrives at it, where the program goes on: at its
    /// caller's return point.
    fn a_method_breakpoint_reports_the_exit_of_a_home_a_block_returns_from(lowering) {
        let ((ends, stood, again), answer) = stepped(
            UNWOUND,
            lowering,
            "run",
            |remote| {
                set_method_breakpoints(remote, vec![method_breakpoint("Unwound", &[], false, "find")]);
            },
            |remote| {
                let id = only_method_breakpoint(&expect_method_breakpoints(remote), true);
                let entry = method_end(expect_stop(remote), id);
                continue_stopped(remote);
                let exit = method_end(expect_stop(remote), id);
                let stood = standing(remote, UNWOUND);
                continue_stopped(remote);
                ((entry, exit), stood, stops_until_the_end(remote))
            },
        );
        assert_eq!(ends, ("entry", "exit"));
        assert_eq!(
            stood,
            (String::from("Unwound>>run"), String::from("found:: self find"))
        );
        assert_eq!(again, 0);
        assert_eq!(answer, 21);
    }
}

ns_runtime::for_each_lowering! {
    /// A method breakpoint on a shipped method with a fast path stops at
    /// its entry: the fast path is off while it stands. Removed, it puts
    /// the fast path back.
    fn a_method_breakpoint_on_a_quick_shipped_method_stops(lowering) {
        let (mut interpreter, class) = load(QUICK, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let quick = Some(ns_runtime::mixin::MethodFastPath::ValueOfParameter(0));
        for count in 1..=5 {
            assert_eq!(send_answered(&mut interpreter, &instance, "run"), count);
        }
        let remote = interpreter.attach_debugger();
        set_method_breakpoints(
            &remote,
            vec![method_breakpoint("Kernel", &["True"], false, "ifTrue:")],
        );
        let ((end, frames), result, _) = debugged_by(
            remote,
            |remote| {
                let id = only_method_breakpoint(&expect_method_breakpoints(remote), true);
                let end = method_end(expect_stop(remote), id);
                let frames: Vec<FrameKind> = stack_trace(remote)
                    .into_iter()
                    .take(2)
                    .map(|frame| frame.kind)
                    .collect();
                set_method_breakpoints(remote, Vec::new());
                assert_eq!(expect_method_breakpoints(remote), []);
                continue_stopped(remote);
                (end, frames)
            },
            || run(&mut interpreter, &instance),
        );
        assert_eq!(end, "entry");
        assert_eq!(frames, [method("ifTrue:"), method("check:with:")]);
        assert_eq!(answered(&mut interpreter, result), 6);
        assert_eq!(if_true_fast_path(&mut interpreter), quick);
    }
}

ns_runtime::for_each_lowering! {
    /// A method breakpoint in a module not loaded yet is not verified;
    /// loading the module resolves it, reported unasked, and the method
    /// then stops at its entry and its exit.
    fn a_pending_method_breakpoint_resolves_when_its_class_is_built(lowering) {
        let (mut interpreter, class) = interpreted(THREE_STATEMENTS, lowering);
        let remote = interpreter.attach_debugger();
        set_method_breakpoints(&remote, vec![method_breakpoint("Later", &[], false, "answer")]);
        expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let id = only_method_breakpoint(&expect_method_breakpoints(&remote), false);
        let later_class = interpreter.load_module(&resolved_module(LATER));
        assert_eq!(only_method_breakpoint(&expect_method_breakpoints(&remote), true), id);
        let later = expect_ok(interpreter.instantiate(&later_class, &[]), "instantiate Later");
        let (stops, result, _) = debugged_by(
            remote,
            move |remote| {
                let mut stops = Vec::new();
                for _ in 0..2 {
                    let end = method_end(expect_stop(remote), id);
                    stops.push((end, top_statement(remote, LATER)));
                    continue_stopped(remote);
                }
                stops
            },
            || interpreter.send(&later, Symbol::intern("answer"), &[]),
        );
        assert_eq!(
            stops,
            [
                ("entry", String::from("value:: 6 * 7")),
                ("exit", String::from("^value")),
            ]
        );
        assert_eq!(answered(&mut interpreter, result), 42);
    }
}

ns_runtime::for_each_lowering! {
    /// A stop that is a method's entry and a line breakpoint's statement
    /// reports the entry, naming the line breakpoint among the others.
    fn a_method_entry_at_a_line_breakpoint_names_both(lowering) {
        let offset = line_start(THREE_STATEMENTS, "count:: count + 1.");
        let ((line, method, reasons), result) = debugged(
            THREE_STATEMENTS,
            lowering,
            |remote| {
                set_breakpoints(remote, "Lines", vec![offset]);
                set_method_breakpoints(remote, vec![method_breakpoint("Lines", &[], false, "run")]);
            },
            |remote| {
                let (line, _) = resolved(&expect_breakpoints(remote, "Lines"));
                let method = only_method_breakpoint(&expect_method_breakpoints(remote), true);
                let mut reasons = Vec::new();
                for _ in 0..2 {
                    reasons.push(expect_stop(remote));
                    continue_stopped(remote);
                }
                (line, method, reasons)
            },
        );
        assert_eq!(
            reasons,
            [
                StopReason::MethodEntry {
                    id: method,
                    others: vec![line]
                },
                StopReason::MethodExit {
                    id: method,
                    others: Vec::new()
                },
            ]
        );
        expect_ok(result, "run");
    }
}

/// A loop summing a lazy slot of an object it holds, whose getter the
/// optimized copy of `loop:` inlines once the slot is filled.
const LAZY: &str = indoc! {"
    class Lazy = Object new (
      | holder = Holder new. |
    ) (
      public class Holder = Object new () (
        lazy public value = 21.
      )
      public loop: turns = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < turns] whileTrue: [
          sum:: sum + holder value.
          index:: index + 1].
        ^sum
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A method breakpoint on a lazy slot's getter, which an optimized copy
    /// inlined without naming its body, frees the copy, and every read
    /// of the filled slot stops at the getter's entry and at its exit,
    /// both at its first instruction, the one that answers a filled slot.
    fn a_method_breakpoint_on_an_inlined_lazy_getter_stops_at_every_read(lowering) {
        let (mut interpreter, class) = load(LAZY, lowering);
        interpreter.set_jit_stress(true);
        interpreter.set_optimize_stress(true);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..3 {
            assert_eq!(loop_answered(&mut interpreter, &instance, 100), 2100);
        }
        if lowering == Lowering::Optimized && counters_are_exact() {
            assert_eq!(
                interpreter.jit_customized_copies(
                    &class,
                    MethodSide::Instance,
                    Symbol::intern("loop:")
                ),
                1,
                "`loop:` runs as a copy"
            );
        }
        let remote = interpreter.attach_debugger();
        set_method_breakpoints(
            &remote,
            vec![method_breakpoint("Lazy", &["Holder"], false, "value")],
        );
        let (events, result, _) =
            events_during(remote, || send_loop(&mut interpreter, &instance, 3));
        assert_eq!(answered(&mut interpreter, result), 63);
        let ends: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                Event::Stopped {
                    reason: StopReason::MethodEntry { .. },
                    ..
                } => Some("entry"),
                Event::Stopped {
                    reason: StopReason::MethodExit { .. },
                    ..
                } => Some("exit"),
                Event::Stopped { .. } => Some("other"),
                _ => None,
            })
            .collect();
        assert_eq!(ends, ["entry", "exit", "entry", "exit", "entry", "exit"]);
    }
}

/// A method that makes a block, loops, and then runs the block, whose
/// non-local return returns from the method, its home.
const ESCAPING: &str = indoc! {"
    class Escaping = Object new () (
      public run = (
        | found |
        found:: self spin.
        ^found + 1
      )
      spin = (
        |
          index ::= 0.
          action ::= nil.
        |
        action:: [:value |
          ^value * 10].
        [index < 100] whileTrue: [
          index:: index + 1].
        action value: 2.
        ^0
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// Under the JIT stress mode a body that exits at once at its method
    /// breakpoint's entry is installed all the same, and its loop's
    /// on-stack replacement runs it in machine code: the cut of its own
    /// block's non-local return, which would return from it as the
    /// stretch's entry frame, leaves it to the unwinder, whose arrival
    /// reports its exit.
    fn a_method_breakpoint_reports_the_exit_of_a_compiled_home_a_block_returns_from(lowering) {
        let (mut interpreter, class) = load(ESCAPING, lowering);
        interpreter.set_jit_stress(true);
        interpreter.set_jit_compile_at_back_jump(1);
        interpreter.set_optimization_budget(i64::MAX);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for _ in 0..2 {
            assert_eq!(send_answered(&mut interpreter, &instance, "run"), 21);
        }
        let remote = interpreter.attach_debugger();
        set_method_breakpoints(
            &remote,
            vec![method_breakpoint("Escaping", &[], false, "spin")],
        );
        let ((ends, stood, again), result, _) = debugged_by(
            remote,
            |remote| {
                let id = only_method_breakpoint(&expect_method_breakpoints(remote), true);
                let entry = method_end(expect_stop(remote), id);
                continue_stopped(remote);
                let exit = method_end(expect_stop(remote), id);
                let stood = standing(remote, ESCAPING);
                continue_stopped(remote);
                ((entry, exit), stood, stops_until_the_end(remote))
            },
            || run(&mut interpreter, &instance),
        );
        assert_eq!(ends, ("entry", "exit"));
        assert_eq!(
            stood,
            (String::from("Escaping>>run"), String::from("found:: self spin"))
        );
        assert_eq!(again, 0);
        assert_eq!(answered(&mut interpreter, result), 21);
    }
}

ns_runtime::for_each_lowering! {
    /// A step into whose push stops at the callee's first statement, where
    /// a line breakpoint stands, reports the breakpoint.
    fn a_step_into_a_callee_s_first_statement_a_breakpoint_stands_at_reports_it(lowering) {
        let calling = line_start(STEPPING, "x:: self compute.");
        let first = line_start(STEPPING, "a:: 3.");
        let ((reason, ids, stood), _) = stepped(
            STEPPING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Stepping", vec![calling, first]),
            |remote| {
                let ids: Vec<BreakpointId> = expect_breakpoints(remote, "Stepping")
                    .iter()
                    .map(|state| state.id)
                    .collect();
                assert_eq!(expect_breakpoint_stop(remote), [ids[0]]);
                remote.send(Command::StepInto);
                match remote.next_event(EVENT_TIMEOUT) {
                    Some(Event::Continued) => {}
                    other => panic!("the step's confirmation was expected, not {other:?}"),
                }
                let reason = expect_stop(remote);
                let stood = standing(remote, STEPPING);
                continue_stopped(remote);
                (reason, ids, stood)
            },
        );
        assert_eq!(reason, StopReason::Breakpoint { ids: vec![ids[1]] });
        assert_eq!(
            stood,
            (String::from("Stepping>>compute"), String::from("a:: 3"))
        );
    }
}

/// A class-side method with two returns, one in a block the optimized
/// lowering inlines.
const SIDED: &str = indoc! {"
    class Sided = Object new () (
      public run = (
        ^(Helper pick: 1) + (Helper pick: 5)
      )
      public class Helper = Object new () () : (
        public pick: number = (
          number > 3 ifTrue: [
            ^number * 2].
          ^number
        )
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A method breakpoint on a class-side method stops at its entry and
    /// at each return it takes, the one in its inlined block included.
    fn a_method_breakpoint_on_a_class_side_method_stops_at_entry_and_each_return(lowering) {
        let (stops, answer) = stepped(
            SIDED,
            lowering,
            "run",
            |remote| {
                set_method_breakpoints(
                    remote,
                    vec![method_breakpoint("Sided", &["Helper"], true, "pick:")],
                );
            },
            |remote| {
                let id = only_method_breakpoint(&expect_method_breakpoints(remote), true);
                let mut stops = Vec::new();
                for _ in 0..4 {
                    let end = method_end(expect_stop(remote), id);
                    stops.push((end, top_statement(remote, SIDED)));
                    continue_stopped(remote);
                }
                assert_eq!(stops_until_the_end(remote), 0);
                stops
            },
        );
        let entry = "number > 3 ifTrue: [\n        ^number * 2]";
        assert_eq!(
            stops,
            [
                ("entry", String::from(entry)),
                ("exit", String::from("^number")),
                ("entry", String::from(entry)),
                ("exit", String::from("^number * 2")),
            ]
        );
        assert_eq!(answer, 11);
    }
}

ns_runtime::for_each_lowering! {
    /// An install replaces a method's body: its method breakpoint is set
    /// again on the new body, which stops at its entry and its exit.
    fn an_install_sets_a_method_breakpoint_again_on_the_new_body(lowering) {
        let (mut interpreter, class) = interpreted(EDITING, lowering);
        let platform = interpreter.platform();
        let instance = expect_ok(
            interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
            "usingPlatform:",
        );
        let ((ends, names, again), result) = debugged_in(
            &mut interpreter,
            &instance,
            |remote| {
                set_method_breakpoints(
                    remote,
                    vec![method_breakpoint("Editing", &["Subject"], false, "answer")],
                );
            },
            |remote| {
                let id = only_method_breakpoint(&expect_method_breakpoints(remote), true);
                let mut ends = Vec::new();
                let mut names = Vec::new();
                for _ in 0..2 {
                    ends.push(method_end(expect_stop(remote), id));
                    names.push(top_name(remote));
                    continue_stopped(remote);
                }
                (ends, names, stops_until_the_end(remote))
            },
        );
        assert_eq!(ends, ["entry", "exit"]);
        assert_eq!(names, ["Subject>>answer", "Subject>>answer"]);
        assert_eq!(again, 0);
        assert_eq!(answered(&mut interpreter, result), 48);
    }
}

ns_runtime::for_each_lowering! {
    /// Sending the method breakpoints already set again changes no slot:
    /// the body the JIT stress mode installed though it exits at once
    /// keeps its machine code, and the method still stops.
    fn resending_unchanged_method_breakpoints_frees_no_machine_code(lowering) {
        let (mut interpreter, instance, _) = compiled(BUMPED, lowering);
        interpreter.set_jit_stress(true);
        assert_eq!(send_answered(&mut interpreter, &instance, "warm"), 2000);
        let bump = || vec![method_breakpoint("Bumped", &[], false, "bump")];
        let remote = interpreter.attach_debugger();
        set_method_breakpoints(&remote, bump());
        let (events, result, remote) = events_during(remote, || run(&mut interpreter, &instance));
        assert_eq!(answered(&mut interpreter, result), 2003);
        assert_eq!(stop_count(&events), 6, "{events:?}");
        let before = interpreter.jit_statistics();
        set_method_breakpoints(&remote, bump());
        let (events, result, _) = events_during(remote, || run(&mut interpreter, &instance));
        assert_eq!(answered(&mut interpreter, result), 2006);
        assert_eq!(stop_count(&events), 6, "{events:?}");
        let after = interpreter.jit_statistics();
        if runs_in_machine_code(lowering) {
            assert_eq!(
                after.freed_bodies, before.freed_bodies,
                "the unchanged method breakpoint freed no code: {before:?} {after:?}"
            );
        }
    }
}

/// Sends `run` to an instance of `source`'s module class under
/// `lowering`, its JIT on, with a debugger attached: `run` is sent twice
/// before, whatever it answers, so that the bodies it runs -- the kernel's
/// handler search among them -- have machine code; `configure` sets the
/// interpreter up and queues its commands before the run, and `agent`
/// serves it from another thread. Answers what `agent` answered and the
/// run's outcome: the small integer it answered, or its error's text.
fn raised<T: Send + 'static>(
    source: &str,
    lowering: Lowering,
    configure: impl FnOnce(&mut Interpreter, &DebugRemote),
    agent: impl FnOnce(&DebugRemote) -> T + Send + 'static,
) -> (T, Result<i64, String>) {
    let (mut interpreter, instance) = instance_of(source, lowering);
    for _ in 0..2 {
        // Warmed whether the run ends normally or with its error.
        drop(run(&mut interpreter, &instance));
    }
    let remote = interpreter.attach_debugger();
    configure(&mut interpreter, &remote);
    let (answer, result, _) = debugged_by(remote, agent, || run(&mut interpreter, &instance));
    let outcome = match result {
        Ok(root) => Ok(small_integer(&mut interpreter, &root)),
        Err(HostSignal::Error(error)) => Err(String::from(error.message.as_str())),
        Err(HostSignal::Return) => panic!("a non-local return escaped"),
    };
    (answer, outcome)
}

/// Sets the exception breakpoints.
fn set_exception_breakpoints(
    remote: &DebugRemote,
    uncaught: bool,
    caught: bool,
    halt: bool,
    classes: Vec<ClassName>,
) {
    remote.send(Command::SetExceptionBreakpoints {
        uncaught,
        caught,
        halt,
        classes,
    });
}

/// Waits for the answer to `SetExceptionBreakpoints`.
fn expect_exception_breakpoints_set(remote: &DebugRemote) {
    match remote.next_event(EVENT_TIMEOUT) {
        Some(Event::ExceptionBreakpointsSet) => {}
        other => panic!("the exception breakpoints' answer was expected, not {other:?}"),
    }
}

/// The stop an uncaught or a caught exception of `class_name` described
/// as `description` makes.
fn exception(class_name: &str, description: &str, caught: bool) -> StopReason {
    StopReason::Exception {
        class_name: String::from(class_name),
        description: String::from(description),
        caught,
        exits: Vec::new(),
    }
}

/// The name of the first frame of `frames` not hidden, after asserting
/// that the frames above it are all hidden and those below it none.
fn first_shown(frames: &[FrameDescription]) -> String {
    let shown = frames
        .iter()
        .position(|frame| !frame.hidden)
        .unwrap_or_else(|| panic!("a frame is shown: {:?}", frame_names(frames)));
    assert!(
        frames[shown..].iter().all(|frame| !frame.hidden),
        "only the frames above the first shown are hidden: {frames:?}"
    );
    frames[shown].name.clone()
}

/// The text of the error a run ended with.
fn error_text(result: HostResult) -> String {
    match result {
        Err(HostSignal::Error(error)) => String::from(error.message.as_str()),
        Err(HostSignal::Return) => panic!("a non-local return escaped"),
        Ok(_) => panic!("the run was expected to end with an error"),
    }
}

/// A block's non-local return from fifty frames of `descend:with:` deep,
/// which returns from `find`, its home; `call:` runs the block, so that a
/// step from it sets its return point there.
const DEEP_UNWIND: &str = indoc! {"
    class DeepUnwind = Object new () (
      public run = (
        | result |
        result:: self find.
        ^result + 1
      )
      find = (
        self descend: 50 with: [:value |
          ^value * 2].
        ^0
      )
      descend: depth with: action = (
        depth = 0 ifTrue: [
          ^self call: action].
        ^(self descend: depth - 1 with: action) + 1
      )
      call: action = (
        ^action value: 7
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step over from a block whose non-local return unwinds through
    /// fifty frames of `descend:with:` goes on from `find`, the home the
    /// unwind stops in, which alone it floods, and stops at `find`'s
    /// return point in `run`: `descend:with:`, whose frames it passes,
    /// keeps its machine code.
    fn an_unwind_while_stepping_floods_only_the_frame_it_stops_in(lowering) {
        let (mut interpreter, instance, class) = compiled(DEEP_UNWIND, lowering);
        let descend = Symbol::intern("descend:with:");
        for _ in 0..3 {
            assert_eq!(send_answered(&mut interpreter, &instance, "run"), 15);
        }
        if runs_in_machine_code(lowering) {
            assert!(
                interpreter.jit_body_offset(&class, MethodSide::Instance, descend).is_some(),
                "`descend:with:` is compiled"
            );
        }
        let remote = interpreter.attach_debugger();
        set_breakpoints(&remote, "DeepUnwind", vec![line_start(DEEP_UNWIND, "^value * 2].")]);
        let (stood, result, _) = debugged_by(
            remote,
            |remote| {
                let (id, _) = resolved(&expect_breakpoints(remote, "DeepUnwind"));
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                step(remote, Command::StepOver);
                let stood = standing(remote, DEEP_UNWIND);
                continue_stopped(remote);
                stood
            },
            || run(&mut interpreter, &instance),
        );
        assert_eq!(answered(&mut interpreter, result), 15);
        assert_eq!(
            stood,
            (String::from("DeepUnwind>>run"), String::from("result:: self find"))
        );
        if runs_in_machine_code(lowering) {
            assert!(
                interpreter.jit_body_offset(&class, MethodSide::Instance, descend).is_some(),
                "no flood of the frames the unwind passed freed `descend:with:`'s code"
            );
        }
    }
}

/// A method that signals an `Error` nothing handles.
const RAISING: &str = indoc! {"
    class Raising = Object new () (
      public run = (
        ^self boom
      )
      boom = (
        Error signal: 'boom'.
        ^1
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// An uncaught error stops where the handler search, having found no
    /// handler, is about to send `defaultAction`, with the whole stack:
    /// the frames of the exception's machinery are hidden and the method
    /// that signalled it is shown first; going on runs `defaultAction`,
    /// which ends the run as it ends without a debugger.
    fn an_uncaught_error_stops_before_its_default_action(lowering) {
        let ((reason, top, shown), result) = raised(
            RAISING,
            lowering,
            |_, _| {},
            |remote| {
                let reason = expect_stop(remote);
                let frames = stack_trace(remote);
                continue_stopped(remote);
                (reason, frames[0].name.clone(), first_shown(&frames))
            },
        );
        assert_eq!(reason, exception("Error", "boom", false));
        assert_eq!(top, "Exception>>defaultAction");
        assert_eq!(shown, "Raising>>boom");
        assert_eq!(result, Err(String::from("boom")));
    }
}

ns_runtime::for_each_lowering! {
    /// With the shipped filter off, the first frame shown is still the
    /// first below the machinery: the step filters only push it further
    /// down, past the frames they filter.
    fn without_the_shipped_filter_the_first_frame_shown_is_below_the_machinery(lowering) {
        let (shown, result) = raised(
            RAISING,
            lowering,
            |_, remote| {
                remote.send(Command::SetStepFilters {
                    shipped: false,
                    classes: Vec::new(),
                });
            },
            |remote| {
                expect_step_filters_set(remote);
                expect_stop(remote);
                let shown = first_shown(&stack_trace(remote));
                continue_stopped(remote);
                shown
            },
        );
        assert_eq!(shown, "Raising>>boom");
        assert_eq!(result, Err(String::from("boom")));
    }
}

/// A method left by the unwind of a handled error, after which the
/// statement the program goes on in raises an error nothing handles
/// before it reaches another statement or a return point.
const LEFT_THEN_RAISING: &str = indoc! {"
    class LeftThenRaising = Object new () (
      public run = (
        ^([self each: [:value | Error signal: 'inner']] on: Error do: [:error | error return: 7])
          bogus
      )
      each: action = (
        action value: 2.
        ^nil
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A method breakpoint's exit by an unwind, waiting for the next
    /// place the program reaches, is reported by an exception's stop
    /// served first, beside the exception, rather than dropped.
    fn an_exception_stop_reports_a_pending_method_exit(lowering) {
        let ((id, reasons), result) = raised(
            LEFT_THEN_RAISING,
            lowering,
            |_, remote| {
                set_method_breakpoints(
                    remote,
                    vec![method_breakpoint("LeftThenRaising", &[], false, "each:")],
                );
            },
            |remote| {
                let id = only_method_breakpoint(&expect_method_breakpoints(remote), true);
                let mut reasons = Vec::new();
                for _ in 0..2 {
                    reasons.push(expect_stop(remote));
                    continue_stopped(remote);
                }
                (id, reasons)
            },
        );
        assert_eq!(
            reasons,
            [
                StopReason::MethodEntry {
                    id,
                    others: Vec::new()
                },
                StopReason::Exception {
                    class_name: String::from("MessageNotUnderstood"),
                    description: String::from("an object does not understand #bogus"),
                    caught: false,
                    exits: vec![id],
                },
            ]
        );
        assert!(result.is_err());
    }
}

/// An error a shipped method a program sent signals.
const ABSTRACT: &str = indoc! {"
    class Abstract = Object new () (
      public run = (
        ^self area
      )
      area = (
        ^self subclassResponsibility
      )
    ) : ()
"};

/// The first frame an uncaught error signalled in `ABSTRACT` shows, the
/// shipped filter as `shipped` says.
fn shown_for_abstract(lowering: Lowering, shipped: bool) -> String {
    let (shown, result) = raised(
        ABSTRACT,
        lowering,
        |_, remote| {
            remote.send(Command::SetStepFilters {
                shipped,
                classes: Vec::new(),
            });
        },
        |remote| {
            expect_step_filters_set(remote);
            expect_stop(remote);
            let shown = first_shown(&stack_trace(remote));
            continue_stopped(remote);
            shown
        },
    );
    assert!(result.is_err(), "the error ends the run");
    shown
}

ns_runtime::for_each_lowering! {
    /// An error raised in a shipped method shows the program's frame that
    /// sent it first, the step filters filtering the shipped one; with the
    /// shipped filter off, the shipped method is shown first.
    fn an_error_raised_in_shipped_code_shows_the_program_s_frame_first(lowering) {
        assert_eq!(shown_for_abstract(lowering, true), "Abstract>>area");
        assert_eq!(
            shown_for_abstract(lowering, false),
            "Object>>subclassResponsibility"
        );
    }
}

/// A handler that takes the `Error` a method signals.
const CATCHING: &str = indoc! {"
    class Catching = Object new () (
      public run = (
        ^[self boom] on: Error do: [:error | 7]
      )
      boom = (
        Error signal: 'boom'.
        ^1
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A caught exception does not stop by default.
    fn a_caught_exception_does_not_stop_by_default(lowering) {
        let (mut interpreter, instance) = instance_of(CATCHING, lowering);
        let remote = interpreter.attach_debugger();
        let (events, result, _) = events_during(remote, || run(&mut interpreter, &instance));
        assert_no_stop(&events);
        assert_eq!(answered(&mut interpreter, result), 7);
    }
}

ns_runtime::for_each_lowering! {
    /// With the caught-exception breakpoint on, a caught exception stops
    /// where the search is about to run the handler it found: the handler
    /// block on top, at its first instruction, is shown first, the
    /// exception's machinery between it and the method that signalled it
    /// hidden, and that method shown next; going on runs the handler.
    fn a_caught_exception_stops_only_with_caught_on(lowering) {
        let ((reason, frames), result) = raised(
            CATCHING,
            lowering,
            |_, remote| set_exception_breakpoints(remote, true, true, true, Vec::new()),
            |remote| {
                expect_exception_breakpoints_set(remote);
                let reason = expect_stop(remote);
                let frames = stack_trace(remote);
                continue_stopped(remote);
                (reason, frames)
            },
        );
        assert_eq!(reason, exception("Error", "boom", true));
        let raising = frame_named(&frames, "Catching>>boom");
        assert_eq!(frames[0].name, "[] in Catching>>run");
        assert_eq!(frames[0].program_counter, 0);
        assert!(!frames[0].hidden, "the handler block is shown");
        assert!(raising > 1, "machinery lies between: {:?}", frame_names(&frames));
        assert!(
            frames[1..raising].iter().all(|frame| frame.hidden),
            "the machinery is hidden: {frames:?}"
        );
        assert!(frames[raising..].iter().all(|frame| !frame.hidden));
        assert_eq!(result, Ok(7));
    }
}

/// A handler block of two statements, which takes the `Error` a method
/// signals inside the protected block of a statement followed by another.
const HANDLING: &str = indoc! {"
    class Handling = Object new () (
      public run = (
        | answer |
        answer:: [self boom] on: Error do: [:error |
          | partial |
          partial:: 3.
          partial + 4].
        ^answer + 1
      )
      boom = (
        Error signal: 'boom'.
        ^1
      )
    ) : ()
"};

/// Takes the step `command` at the caught stop of `HANDLING` and answers
/// the frame and statement it stops at, and the run's outcome.
fn stepped_from_a_catch(
    lowering: Lowering,
    command: Command,
) -> ((String, String), Result<i64, String>) {
    raised(
        HANDLING,
        lowering,
        |_, remote| set_exception_breakpoints(remote, true, true, true, Vec::new()),
        move |remote| {
            expect_exception_breakpoints_set(remote);
            assert_eq!(expect_stop(remote), exception("Error", "boom", true));
            step(remote, command);
            let stood = standing(remote, HANDLING);
            continue_stopped(remote);
            stood
        },
    )
}

ns_runtime::for_each_lowering! {
    /// A step over at a caught exception's stop steps in the handler
    /// block it stands in: it stops at the handler's second statement.
    fn a_step_over_from_a_caught_exception_stops_in_the_handler(lowering) {
        let (stood, result) = stepped_from_a_catch(lowering, Command::StepOver);
        assert_eq!(
            stood,
            (String::from("[] in Handling>>run"), String::from("partial + 4"))
        );
        assert_eq!(result, Ok(8));
    }
}

ns_runtime::for_each_lowering! {
    /// A step out at a caught exception's stop goes out of the handler
    /// block, past the whole `on:do:` that took it, to where the `on:do:`
    /// returns: mid-statement, its answer yet to be assigned.
    fn a_step_out_from_a_caught_exception_stops_after_the_on_do(lowering) {
        let (stood, result) = stepped_from_a_catch(lowering, Command::StepOut);
        let from = HANDLING.find("answer:: [").expect("the on:do:'s statement");
        let until = HANDLING.find("^answer + 1").expect("the next statement");
        let statement = HANDLING[from..until].trim_end().trim_end_matches('.');
        assert_eq!(
            stood,
            (String::from("Handling>>run"), String::from(statement))
        );
        assert_eq!(result, Ok(8));
    }
}

/// Two caught exceptions: an `Error`, and one of a program class whose
/// superclass is another program class.
const CLASSED: &str = indoc! {"
    class Classed = Object new () (
      public run = (
        | total |
        total:: [Error new signal: 'plain'] on: Error do: [:error | 1].
        total:: total + ([MoreSpecial new signal: 'more'] on: Error do: [:error | 10]).
        ^total
      )
      class Special = Error () ()
      class MoreSpecial = Special () ()
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A class an exception breakpoint names takes an exception whose
    /// class inherits from it, matched by declaration along the class
    /// chain, and no other: the plain `Error` runs on.
    fn an_exception_class_filter_selects_by_class_chain(lowering) {
        let special = ClassName {
            module: String::from("Classed"),
            path: vec![String::from("Special")],
        };
        let ((reason, after), result) = raised(
            CLASSED,
            lowering,
            |_, remote| set_exception_breakpoints(remote, true, true, true, vec![special]),
            |remote| {
                expect_exception_breakpoints_set(remote);
                let reason = expect_stop(remote);
                continue_stopped(remote);
                (reason, remote.next_event(Duration::from_millis(500)))
            },
        );
        assert_eq!(reason, exception("MoreSpecial", "more", true));
        assert_eq!(after, None);
        assert_eq!(result, Ok(11));
    }
}

/// A `halt` whose answer the method goes on with.
const HALTING: &str = indoc! {"
    class Halting = Object new () (
      public run = (
        | answer |
        answer:: self halt.
        ^answer isNil ifTrue: [5] ifFalse: [6]
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// An uncaught `halt` stops under the `Halt` breakpoint, on by
    /// default, the method that sent it shown first; going on resumes
    /// the `Halt` with nil, which `halt` answers, and the run goes on.
    fn halt_stops_and_continues_with_nil(lowering) {
        let ((reason, shown), result) = raised(
            HALTING,
            lowering,
            |_, _| {},
            |remote| {
                let reason = expect_stop(remote);
                let shown = first_shown(&stack_trace(remote));
                continue_stopped(remote);
                (reason, shown)
            },
        );
        assert_eq!(reason, StopReason::Halt { exits: Vec::new() });
        assert_eq!(shown, "Halting>>run");
        assert_eq!(result, Ok(5));
    }
}

/// A `halt` between two statements of a method a caller sent.
const HALTING_BETWEEN: &str = indoc! {"
    class Pausing = Object new () (
      public run = (
        | x |
        x:: self middle.
        ^x + 1
      )
      middle = (
        | x |
        x:: 3.
        self halt.
        self note: x.
        ^x
      )
      note: value = (
        ^value
      )
    ) : ()
"};

/// Takes the step `command` at the `Halt` stop of `HALTING_BETWEEN` and
/// answers the frame and statement it stops at, and the run's outcome.
fn stepped_from_halt(
    lowering: Lowering,
    command: Command,
) -> ((String, String), Result<i64, String>) {
    raised(
        HALTING_BETWEEN,
        lowering,
        |_, _| {},
        move |remote| {
            assert_eq!(expect_stop(remote), StopReason::Halt { exits: Vec::new() });
            step(remote, command);
            let stood = standing(remote, HALTING_BETWEEN);
            continue_stopped(remote);
            stood
        },
    )
}

ns_runtime::for_each_lowering! {
    /// A step over at a `Halt` stop steps from the frame the stop shows
    /// first, the one that sent `halt`, never in the kernel's machinery the
    /// stop stands in: it stops at that frame's next statement.
    fn a_step_over_from_a_halt_stops_at_the_next_statement(lowering) {
        let (stood, result) = stepped_from_halt(lowering, Command::StepOver);
        assert_eq!(
            stood,
            (String::from("Pausing>>middle"), String::from("self note: x"))
        );
        assert_eq!(result, Ok(4));
    }
}

ns_runtime::for_each_lowering! {
    /// A step into at a `Halt` stop stops where a step into from the
    /// statement `self halt` would: the `halt`'s continuation pushes no
    /// program code, so at the next statement.
    fn a_step_into_from_a_halt_stops_at_the_next_statement(lowering) {
        let (stood, result) = stepped_from_halt(lowering, Command::StepInto);
        assert_eq!(
            stood,
            (String::from("Pausing>>middle"), String::from("self note: x"))
        );
        assert_eq!(result, Ok(4));
    }
}

ns_runtime::for_each_lowering! {
    /// A step out at a `Halt` stop goes out of the frame that sent `halt`,
    /// to its caller's return point.
    fn a_step_out_from_a_halt_stops_in_the_caller(lowering) {
        let (stood, result) = stepped_from_halt(lowering, Command::StepOut);
        assert_eq!(
            stood,
            (String::from("Pausing>>run"), String::from("x:: self middle"))
        );
        assert_eq!(result, Ok(4));
    }
}

ns_runtime::for_each_lowering! {
    /// A step over at an uncaught error's stop steps from the frame that
    /// signalled it, never in `defaultAction`, which ends the run: no stop
    /// follows.
    fn a_step_over_from_an_uncaught_error_goes_where_the_program_goes(lowering) {
        let (stops, result) = raised(
            RAISING,
            lowering,
            |_, _| {},
            |remote| {
                expect_stop(remote);
                remote.send(Command::StepOver);
                match remote.next_event(EVENT_TIMEOUT) {
                    Some(Event::Continued) => {}
                    other => panic!("the confirmation of the step was expected, not {other:?}"),
                }
                stops_until_the_end(remote)
            },
        );
        assert_eq!(stops, 0);
        assert_eq!(result, Err(String::from("boom")));
    }
}

/// An error whose class's `defaultAction`, of two statements, answers, so
/// that its `signal:` does and the method goes on.
const SOFTLY_RAISING: &str = indoc! {"
    class SoftlyRaising = Object new () (
      public run = (
        ^self boom
      )
      boom = (
        | x |
        x:: Soft new signal: 'soft'.
        ^x + 1
      )
      class Soft = Error () (
        public defaultAction = (
          | answer |
          answer:: 40.
          ^answer + 1
        )
      ) : ()
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step over at an uncaught error's stop whose default action
    /// answers stops at the next statement of the frame that signalled it,
    /// not in the default action the stop stands in, a program's method.
    fn a_step_over_from_an_error_a_default_action_answers_stops_at_the_next_statement(lowering) {
        let (stood, result) = raised(
            SOFTLY_RAISING,
            lowering,
            |_, _| {},
            |remote| {
                expect_stop(remote);
                step(remote, Command::StepOver);
                let stood = standing(remote, SOFTLY_RAISING);
                continue_stopped(remote);
                stood
            },
        );
        assert_eq!(
            stood,
            (String::from("SoftlyRaising>>boom"), String::from("^x + 1"))
        );
        assert_eq!(result, Ok(42));
    }
}

ns_runtime::for_each_lowering! {
    /// With the `Halt` breakpoint off, an uncaught `Halt` is an uncaught
    /// exception as any other: it stops as one while the
    /// uncaught-exception breakpoint is on, and, going on, ends the run as
    /// it does without a debugger.
    fn with_the_halt_breakpoint_off_a_halt_is_an_uncaught_exception(lowering) {
        let (reason, result) = raised(
            HALTING,
            lowering,
            |_, remote| set_exception_breakpoints(remote, true, false, false, Vec::new()),
            |remote| {
                expect_exception_breakpoints_set(remote);
                let reason = expect_stop(remote);
                continue_stopped(remote);
                reason
            },
        );
        assert_eq!(reason, exception("Halt", "Halt", false));
        assert_eq!(result, Err(String::from("Halt")));
    }
}

/// A message nothing understands.
const MISSING: &str = indoc! {"
    class Missing = Object new () (
      public run = (
        ^self ask
      )
      ask = (
        ^3 frobnicate
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// An uncaught `MessageNotUnderstood` shows first the sender of the
    /// message, below the kernel's `doesNotUnderstand:` that signalled
    /// it, the shipped filter off so that the machinery alone decides;
    /// going on ends the run as it does without a debugger.
    fn an_uncaught_message_not_understood_stops_below_does_not_understand(lowering) {
        let ((reason, shown), result) = raised(
            MISSING,
            lowering,
            |_, remote| {
                remote.send(Command::SetStepFilters {
                    shipped: false,
                    classes: Vec::new(),
                });
            },
            |remote| {
                expect_step_filters_set(remote);
                let reason = expect_stop(remote);
                let shown = first_shown(&stack_trace(remote));
                continue_stopped(remote);
                (reason, shown)
            },
        );
        assert_eq!(
            reason,
            exception(
                "MessageNotUnderstood",
                "an object does not understand #frobnicate",
                false
            )
        );
        assert_eq!(shown, "Missing>>ask");
        assert_eq!(
            result,
            Err(String::from("an object does not understand #frobnicate"))
        );
    }
}

/// The golden whose `Halt`s are caught and resumed, then one is not.
const HALTS: &str = include_str!(
    "../../../goldens/library/exception/halt_resumes_when_handled_and_ends_the_run_when_not.ns"
);

ns_runtime::for_each_lowering! {
    /// With a debugger attached and every exception breakpoint off, a
    /// golden that catches, resumes and leaves uncaught a `Halt` runs as
    /// it does without one: no stop, and the run ends with the error its
    /// record holds. Without a debugger every golden under
    /// `goldens/library/exception/` keeps its record in the default suite.
    fn without_a_debugger_exceptions_behave_as_before(lowering) {
        let (mut interpreter, class) = load(HALTS, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let platform = interpreter.platform();
        let arguments = interpreter.stash(ns_runtime::Value::nil());
        let remote = interpreter.attach_debugger();
        set_exception_breakpoints(&remote, false, false, false, Vec::new());
        let (events, result, _) = events_during(remote, || {
            interpreter.send(
                &instance,
                Symbol::intern("main:args:"),
                &[platform, arguments],
            )
        });
        assert_no_stop(&events);
        assert_eq!(error_text(result), "Halt");
    }
}

/// A block whose `^` cannot return, its home gone, sent in a protected
/// block whose handler takes the `CannotReturn` the VM raises.
const CANNOT_RETURNING: &str = indoc! {"
    class Escaping = Object new (
      | count ::= 0. |
    ) (
      public run = (
        | block |
        block:: self escaper.
        [block value: 3] on: Error do: [:error |
          count:: count + 5].
        ^count
      )
      escaper = (
        ^[:value |
          count:: value.
          ^value]
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step over a statement whose own instruction raises an error the
    /// VM signals -- the `^` of a block whose home has returned, no send
    /// -- that a handler below the stepping frame takes stops at the
    /// handler block's first statement.
    fn a_step_over_an_instruction_that_raises_stops_in_the_handler_block(lowering) {
        let offset = line_start(CANNOT_RETURNING, "count:: value.");
        let (stood, answer) = stepped(
            CANNOT_RETURNING,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "Escaping", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Escaping");
                step(remote, Command::StepOver);
                let before = standing(remote, CANNOT_RETURNING);
                step(remote, Command::StepOver);
                let stood = standing(remote, CANNOT_RETURNING);
                continue_stopped(remote);
                (before, stood)
            },
        );
        assert_eq!(
            stood,
            (
                (String::from("[] in Escaping>>escaper"), String::from("^value")),
                (
                    String::from("[] in Escaping>>run"),
                    String::from("count:: count + 5")
                )
            )
        );
        assert_eq!(answer, 8);
    }
}

/// `CANNOT_RETURNING`'s block, its `CannotReturn` handled by nothing, below an
/// `ensure:` whose block the termination's unwind runs.
const RESUMED: &str = indoc! {"
    class Resumed = Object new (
      | count ::= 0. |
    ) (
      public run = (
        [self escaper value: 3] ensure: [
          count:: count + 1].
        ^count
      )
      escaper = (
        ^[:value |
          count:: value.
          ^value]
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step out of the `signal` the VM pushed for an error, which
    /// returns once the test hook resumed its default action -- its
    /// continuation ends the run (`Continuation::VmErrorSignalled`) --
    /// goes on where the run's end takes the program: into the `ensure:`
    /// block its termination runs. The uncaught-exception breakpoint does
    /// not stop the exception the hook resumes.
    fn a_step_out_of_a_vm_error_s_signal_stops_in_the_ensure_block_its_end_runs(lowering) {
        let offset = line_start(RESUMED, "count:: value.");
        let (stood, result) = raised(
            RESUMED,
            lowering,
            |interpreter, remote| {
                interpreter.resume_unhandled_exceptions(1);
                remote.send(Command::SetStepFilters {
                    shipped: false,
                    classes: Vec::new(),
                });
                set_breakpoints(remote, "Resumed", vec![offset]);
            },
            |remote| {
                expect_step_filters_set(remote);
                expect_first_stop(remote, "Resumed");
                step(remote, Command::StepOver);
                step(remote, Command::StepInto);
                let signal = top_name(remote);
                step(remote, Command::StepOut);
                let stood = standing(remote, RESUMED);
                continue_stopped(remote);
                (signal, stood)
            },
        );
        assert_eq!(
            stood,
            (
                String::from("Exception>>signal"),
                (
                    String::from("[] in Resumed>>run"),
                    String::from("count:: count + 1")
                )
            )
        );
        assert_eq!(
            result,
            Err(String::from(
                "cannotReturn: the enclosing method activation has already returned"
            ))
        );
    }
}

/// A handler block in the method a step is taken in, whose `on:do:` the
/// method sends.
const HANDLING_HERE: &str = indoc! {"
    class HandlingHere = Object new () (
      public run = (
        | total |
        total:: [self boom] on: Error do: [:error | 7].
        ^total + 1
      )
      boom = (
        Error signal: 'boom'.
        ^1
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A step over a statement whose `on:do:` the stepping frame sends
    /// itself goes on past the handler, which runs above that `on:do:`,
    /// to the frame's next statement: the handler search reports only to
    /// a step whose frame stands at or above the handler's `on:do:`.
    fn a_step_over_a_statement_whose_handler_the_frame_sends_stops_at_its_next_statement(lowering) {
        let offset = line_start(HANDLING_HERE, "total:: [self boom]");
        let (stood, answer) = stepped(
            HANDLING_HERE,
            lowering,
            "run",
            |remote| set_breakpoints(remote, "HandlingHere", vec![offset]),
            |remote| {
                expect_first_stop(remote, "HandlingHere");
                step(remote, Command::StepOver);
                let stood = standing(remote, HANDLING_HERE);
                continue_stopped(remote);
                stood
            },
        );
        assert_eq!(
            stood,
            (
                String::from("HandlingHere>>run"),
                String::from("^total + 1")
            )
        );
        assert_eq!(answer, 8);
    }
}

/// An exception class whose `defaultAction` a fast path answers, with no
/// activation of its own.
const QUIET: &str = indoc! {"
    class Quiet = Object new () (
      public run = (
        ^self raise + 1
      )
      raise = (
        ^Hushed new signal
      )
      class Hushed = Error () (
        public defaultAction = (
          ^41
        )
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// An uncaught exception whose `defaultAction` pushes nothing stops
    /// all the same, at the next poll -- the return to `signal`, the
    /// override's answer already given -- the method that signalled it
    /// shown first; going on, the run answers as it does without a
    /// debugger.
    fn an_uncaught_exception_whose_default_action_pushes_nothing_stops_at_the_next_poll(lowering) {
        let ((reason, shown), result) = raised(
            QUIET,
            lowering,
            |_, _| {},
            |remote| {
                let reason = expect_stop(remote);
                let shown = first_shown(&stack_trace(remote));
                continue_stopped(remote);
                (reason, shown)
            },
        );
        assert_eq!(reason, exception("Hushed", "Hushed", false));
        assert_eq!(shown, "Quiet>>raise");
        assert_eq!(result, Ok(42));
    }
}

ns_runtime::for_each_lowering! {
    /// A caught exception's stop waits for the handler: the frame on top
    /// is the handler block's, at its first instruction, before it ran,
    /// past the kernel's `cull:` that sends it.
    fn a_caught_exception_stops_at_the_handler_block_s_first_instruction(lowering) {
        let ((name, program_counter), result) = raised(
            CATCHING,
            lowering,
            |_, remote| set_exception_breakpoints(remote, true, true, true, Vec::new()),
            |remote| {
                expect_exception_breakpoints_set(remote);
                expect_stop(remote);
                let frames = stack_trace(remote);
                continue_stopped(remote);
                (frames[0].name.clone(), frames[0].program_counter)
            },
        );
        assert_eq!(name, "[] in Catching>>run");
        assert_eq!(program_counter, 0);
        assert_eq!(result, Ok(7));
    }
}

ns_runtime::for_each_lowering! {
    /// Under the JIT stress mode, which compiles every body at its first
    /// entry, the handler search and the method that signals run in
    /// machine code, whose calls of the two leaves do not exit: an
    /// uncaught exception still stops before `defaultAction`'s first
    /// instruction, and a caught one before the handler block's, the
    /// compiled prologue refusing the push under the lowered depth limit.
    fn in_machine_code_an_exception_stops_before_the_first_instruction(lowering) {
        let ((name, program_counter), result) = raised(
            RAISING,
            lowering,
            |interpreter, _| interpreter.set_jit_stress(true),
            |remote| {
                expect_stop(remote);
                let frames = stack_trace(remote);
                continue_stopped(remote);
                (frames[0].name.clone(), frames[0].program_counter)
            },
        );
        assert_eq!(name, "Exception>>defaultAction");
        assert_eq!(program_counter, 0);
        assert_eq!(result, Err(String::from("boom")));
        let ((name, program_counter), result) = raised(
            CATCHING,
            lowering,
            |interpreter, remote| {
                interpreter.set_jit_stress(true);
                set_exception_breakpoints(remote, true, true, true, Vec::new());
            },
            |remote| {
                expect_exception_breakpoints_set(remote);
                expect_stop(remote);
                let frames = stack_trace(remote);
                continue_stopped(remote);
                (frames[0].name.clone(), frames[0].program_counter)
            },
        );
        assert_eq!(name, "[] in Catching>>run");
        assert_eq!(program_counter, 0);
        assert_eq!(result, Ok(7));
    }
}

/// A caught error whose handler is no block but an object whose `cull:`
/// is a slot's setter, which answers without pushing a frame.
const SLOT_HANDLED: &str = indoc! {"
    class SlotHandled = Object new () (
      public run = (
        | result |
        result:: [self raise] on: Error do: Handler new.
        ^7
      )
      raise = (
        ^Error signal: 'slot'
      )
      public class Handler = Object new (
        | public cull ::= nil. |
      ) ()
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A caught exception's stop whose handler pushed no frame -- its
    /// `cull:` a slot's setter -- stands in the kernel's machinery, which
    /// it hides, showing first the program frame below it, as an uncaught
    /// one's does; a step over taken there steps from that frame, never
    /// in the machinery, and stops where the program goes on in it.
    fn a_step_from_a_caught_stop_whose_handler_pushed_nothing_steps_from_the_program(lowering) {
        let ((shown, stood), result) = raised(
            SLOT_HANDLED,
            lowering,
            |_, remote| set_exception_breakpoints(remote, true, true, true, Vec::new()),
            |remote| {
                expect_exception_breakpoints_set(remote);
                assert_eq!(expect_stop(remote), exception("Error", "slot", true));
                let frames = stack_trace(remote);
                let shown = first_shown(&frames);
                step(remote, Command::StepOver);
                let stood = standing(remote, SLOT_HANDLED);
                continue_stopped(remote);
                (shown, stood)
            },
        );
        assert_eq!(shown, "SlotHandled>>raise");
        assert_eq!(
            stood,
            (String::from("SlotHandled>>run"), String::from("^7"))
        );
        assert_eq!(result, Ok(7));
    }
}

/// A caught error that is its own handler: its class declares a slot
/// `cull`, whose setter answers the handler's `cull:` without pushing a
/// frame, and the search then sends `return:` to it.
const SELF_HANDLED: &str = indoc! {"
    class SelfHandled = Object new () (
      public run = (
        | problem result |
        problem:: Selfish new.
        result:: [self raise: problem] on: Selfish do: problem.
        ^7
      )
      raise: problem = (
        ^problem signal
      )
      public class Selfish = Error (
        | public cull ::= nil. |
      ) ()
    ) : ()
"};

/// The first frame shown and where a step over stands at the caught stop
/// of `SELF_HANDLED`, the JIT's stress modes as `stressed` says, and the
/// run's outcome.
fn self_handled_stop(
    lowering: Lowering,
    stressed: bool,
) -> ((String, (String, String)), Result<i64, String>) {
    raised(
        SELF_HANDLED,
        lowering,
        move |interpreter, remote| {
            if stressed {
                interpreter.set_jit_stress(true);
                interpreter.set_optimize_stress(true);
            }
            set_exception_breakpoints(remote, true, true, true, Vec::new());
        },
        |remote| {
            expect_exception_breakpoints_set(remote);
            let reason = expect_stop(remote);
            assert!(
                matches!(reason, StopReason::Exception { caught: true, .. }),
                "a caught exception's stop: {reason:?}"
            );
            let shown = first_shown(&stack_trace(remote));
            step(remote, Command::StepOver);
            let stood = standing(remote, SELF_HANDLED);
            continue_stopped(remote);
            (shown, stood)
        },
    )
}

ns_runtime::for_each_lowering! {
    /// An exception that is its own handler pushes no frame for its
    /// `cull:`, a slot's setter: the frame the search pushes next -- its
    /// `return:`, sent to the same object -- is no frame the handler
    /// pushed, so the stop hides the machinery, shows the program frame
    /// below it first and steps from there.
    fn an_exception_that_is_its_own_handler_steps_from_the_program(lowering) {
        let ((shown, stood), result) = self_handled_stop(lowering, false);
        assert_eq!(shown, "SelfHandled>>raise:");
        assert_eq!(stood, (String::from("SelfHandled>>run"), String::from("^7")));
        assert_eq!(result, Ok(7));
    }
}

ns_runtime::for_each_lowering! {
    /// The same under the JIT's stress modes, whose optimized code may
    /// inline the search's own sends, so that the first poll after the
    /// setter is the push of `return:`.
    fn an_exception_that_is_its_own_handler_steps_from_the_program_in_machine_code(lowering) {
        let ((shown, stood), result) = self_handled_stop(lowering, true);
        assert_eq!(shown, "SelfHandled>>raise:");
        assert_eq!(stood, (String::from("SelfHandled>>run"), String::from("^7")));
        assert_eq!(result, Ok(7));
    }
}

/// A caught error whose handler is an object with a `cull:` method of its
/// own, which pushes a frame.
const METHOD_HANDLED: &str = indoc! {"
    class MethodHandled = Object new () (
      public run = (
        | result |
        result:: [self raise] on: Error do: Answering new.
        ^result
      )
      raise = (
        ^Error signal: 'method'
      )
      public class Answering = Object new () (
        public cull: exception = (
          | answer |
          answer:: 5.
          ^answer
        )
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A handler object's `cull:` method is a frame the handler pushed:
    /// the caught stop stands in it, shown first on top above the hidden
    /// machinery, and a step over is taken in it.
    fn a_caught_stop_stands_in_a_handler_object_s_cull(lowering) {
        let ((shown, stood), result) = raised(
            METHOD_HANDLED,
            lowering,
            |_, remote| set_exception_breakpoints(remote, true, true, true, Vec::new()),
            |remote| {
                expect_exception_breakpoints_set(remote);
                assert_eq!(expect_stop(remote), exception("Error", "method", true));
                let frames = stack_trace(remote);
                assert!(frames[1].hidden, "the machinery below the handler is hidden");
                let shown = frames[0].name.clone();
                step(remote, Command::StepOver);
                let stood = standing(remote, METHOD_HANDLED);
                continue_stopped(remote);
                (shown, stood)
            },
        );
        assert_eq!(shown, "Answering>>cull:");
        assert_eq!(
            stood,
            (String::from("Answering>>cull:"), String::from("^answer"))
        );
        assert_eq!(result, Ok(5));
    }
}

/// A block's `^` whose home has returned, which nothing handles: the VM
/// raises the `CannotReturn` at the block's own instruction.
const STRANDED: &str = indoc! {"
    class Stranded = Object new () (
      public run = (
        ^self escaper value: 3
      )
      escaper = (
        ^[:value | ^value]
      )
    ) : ()
"};

/// The first frame an uncaught `CannotReturn` in `STRANDED` shows, the
/// shipped filter as `shipped` says.
fn shown_for_stranded(lowering: Lowering, shipped: bool) -> String {
    let (shown, result) = raised(
        STRANDED,
        lowering,
        |_, remote| {
            remote.send(Command::SetStepFilters {
                shipped,
                classes: Vec::new(),
            });
        },
        |remote| {
            expect_step_filters_set(remote);
            expect_stop(remote);
            let shown = first_shown(&stack_trace(remote));
            continue_stopped(remote);
            shown
        },
    );
    assert!(result.is_err(), "the error ends the run");
    shown
}

ns_runtime::for_each_lowering! {
    /// An uncaught error the VM raises shows the raising frame first, the
    /// `signal` it pushed right above that frame hidden, with the shipped
    /// filter on and off.
    fn an_uncaught_vm_error_shows_the_raising_frame_first(lowering) {
        assert_eq!(shown_for_stranded(lowering, true), "[] in Stranded>>escaper");
        assert_eq!(shown_for_stranded(lowering, false), "[] in Stranded>>escaper");
    }
}

/// A method stopped at a breakpoint below two `ensure:` blocks, one that
/// signals an `Error` nothing handles and one that sends `halt`.
const ENDING: &str = indoc! {"
    class Ending = Object new (
      | count ::= 0. |
    ) (
      public run = (
        [
          [self bump] ensure: [Error signal: 'late'].
        ] ensure: [self halt].
        ^count
      )
      bump = (
        count:: count + 1.
        ^count
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A termination's unwind runs the pending `ensure:` blocks, and an
    /// uncaught exception or a `halt` one of them raises stops nothing:
    /// no stop arrives after the `Terminate`. Each such exception's
    /// default action ends the run in turn, as it does without a
    /// debugger, so the host receives the last one's error, the `Halt`.
    fn a_termination_s_ensure_blocks_stop_at_no_exception(lowering) {
        let offset = line_start(ENDING, "count:: count + 1.");
        let (after, result) = debugged(
            ENDING,
            lowering,
            |remote| set_breakpoints(remote, "Ending", vec![offset]),
            |remote| {
                expect_first_stop(remote, "Ending");
                remote.send(Command::Terminate);
                let after = remote.next_event(Duration::from_millis(500));
                // A stop that should not have come is let go, so that the
                // test fails on it rather than waiting.
                if let Some(Event::Stopped { .. }) = after {
                    remote.send(Command::Continue);
                }
                after
            },
        );
        assert_eq!(after, None);
        match result {
            Err(HostSignal::Error(error)) => assert_eq!(error.message.as_str(), "Halt"),
            Err(HostSignal::Return) => panic!("a non-local return escaped"),
            Ok(_) => panic!("the run was expected to end with an error"),
        }
    }
}

/// A loop that hands each of its five indexes to a method of its own.
const SINKING: &str = indoc! {"
    class Sinking = Object new (
      | last ::= 0. |
    ) (
      public run = (
        1 to: 5 do: [:index |
          sink: index].
        ^last
      )
      sink: value = (
        last:: value
      )
    ) : ()
"};

/// The `index` of the loop's every stop at the line breakpoint `id`, the
/// run continued after each, until the run ends; the run must stop for
/// nothing else.
fn indexes_at_stops(remote: &DebugRemote, id: BreakpointId) -> Vec<String> {
    let mut indexes = Vec::new();
    loop {
        let ended = run_finished();
        match remote.next_event(Duration::from_millis(10)) {
            Some(Event::Stopped {
                reason: StopReason::Breakpoint { ids },
                returned: None,
            }) => {
                assert_eq!(ids, [id]);
                let variables = variables(remote, Command::Variables { frame: 0 });
                indexes.push(value_of(&variables, "index").label.clone());
                continue_stopped(remote);
            }
            Some(other) => panic!("only stops at the breakpoint were expected, not {other:?}"),
            None if ended => return indexes,
            None => {}
        }
    }
}

/// The `index` of the loop's every stop at a line breakpoint on `sink:`
/// whose hit condition is `condition`.
fn indexes_stopped_at(lowering: Lowering, condition: HitCondition) -> Vec<String> {
    let offset = line_start(SINKING, "sink: index].");
    let (indexes, result) = debugged(
        SINKING,
        lowering,
        move |remote| {
            set_line_breakpoints(
                remote,
                "Sinking",
                vec![LineBreakpoint {
                    offset,
                    hit_condition: Some(condition),
                }],
            );
        },
        |remote| {
            let (id, _) = resolved(&expect_breakpoints(remote, "Sinking"));
            indexes_at_stops(remote, id)
        },
    );
    expect_ok(result, "run");
    indexes
}

ns_runtime::for_each_lowering! {
    /// A breakpoint whose hit condition names one hit stops at that hit
    /// only: the VM counts the others and goes past them.
    fn a_breakpoint_with_a_count_stops_at_that_hit_only(lowering) {
        assert_eq!(indexes_stopped_at(lowering, HitCondition::Equal(3)), ["3"]);
    }
}

ns_runtime::for_each_lowering! {
    /// A breakpoint whose hit condition is a lower bound stops at that hit
    /// and at every one after it.
    fn a_breakpoint_greater_or_equal_stops_from_that_hit_on(lowering) {
        assert_eq!(
            indexes_stopped_at(lowering, HitCondition::GreaterOrEqual(4)),
            ["4", "5"]
        );
    }
}

ns_runtime::for_each_lowering! {
    /// A breakpoint whose hit condition is a multiple stops at every hit
    /// whose number it divides.
    fn a_breakpoint_multiple_stops_at_every_nth_hit(lowering) {
        assert_eq!(
            indexes_stopped_at(lowering, HitCondition::Multiple(2)),
            ["2", "4"]
        );
    }
}

ns_runtime::for_each_lowering! {
    /// Sending a module's breakpoints again sets each afresh, its count
    /// back at none: the frame standing at the breakpoint goes past it
    /// once, uncounted, so that the next hit is the first of the new count.
    fn setting_breakpoints_again_restarts_their_count(lowering) {
        let offset = line_start(SINKING, "sink: index].");
        let breakpoints = vec![LineBreakpoint {
            offset,
            hit_condition: Some(HitCondition::Equal(2)),
        }];
        let configured = breakpoints.clone();
        let ((first, second), result) = debugged(
            SINKING,
            lowering,
            move |remote| set_line_breakpoints(remote, "Sinking", configured),
            move |remote| {
                let (id, _) = resolved(&expect_breakpoints(remote, "Sinking"));
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                let variables = variables(remote, Command::Variables { frame: 0 });
                let first = value_of(&variables, "index").label.clone();
                set_line_breakpoints(remote, "Sinking", breakpoints);
                let (again, _) = resolved(&expect_breakpoints(remote, "Sinking"));
                continue_stopped(remote);
                (first, indexes_at_stops(remote, again))
            },
        );
        expect_ok(result, "run");
        assert_eq!(first, "2");
        assert_eq!(second, ["4"]);
    }
}

ns_runtime::for_each_lowering! {
    /// A method breakpoint counts each entry and each exit of its method
    /// as a hit: the second is the first call's exit.
    fn a_method_breakpoint_counts_entries_and_exits(lowering) {
        let (mut interpreter, class) = interpreted(SINKING, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let ((reason, value, later), result) = debugged_in(
            &mut interpreter,
            &instance,
            |remote| {
                let mut breakpoint = method_breakpoint("Sinking", &[], false, "sink:");
                breakpoint.hit_condition = Some(HitCondition::Equal(2));
                set_method_breakpoints(remote, vec![breakpoint]);
            },
            |remote| {
                let id = only_method_breakpoint(&expect_method_breakpoints(remote), true);
                let reason = expect_stop(remote);
                let variables = variables(remote, Command::Variables { frame: 0 });
                let value = value_of(&variables, "value").label.clone();
                continue_stopped(remote);
                ((reason, id), value, stops_until_the_end(remote))
            },
        );
        let (reason, id) = reason;
        assert_eq!(reason, StopReason::MethodExit { id, others: Vec::new() });
        assert_eq!(value, "1");
        assert_eq!(later, 0);
        assert_eq!(answered(&mut interpreter, result), 5);
    }
}

ns_runtime::for_each_lowering! {
    /// A step's stop at a breakpoint's statement is the step's whatever
    /// the breakpoint's hit condition says: the reach is counted, and a
    /// condition that does not hold there neither suppresses the stop nor
    /// is named by it.
    fn a_step_stops_at_a_breakpoint_whose_condition_does_not_hold(lowering) {
        let first = line_start(THREE_STATEMENTS, "count:: count + 1.");
        let second = line_start(THREE_STATEMENTS, "count:: count + 10.");
        let ((reason, statement), result) = debugged(
            THREE_STATEMENTS,
            lowering,
            |remote| {
                set_line_breakpoints(
                    remote,
                    "Lines",
                    vec![
                        LineBreakpoint::at(first),
                        LineBreakpoint {
                            offset: second,
                            hit_condition: Some(HitCondition::Equal(2)),
                        },
                    ],
                );
            },
            |remote| {
                let breakpoints = expect_breakpoints(remote, "Lines");
                assert_eq!(expect_breakpoint_stop(remote), [breakpoints[0].id]);
                remote.send(Command::StepOver);
                match remote.next_event(EVENT_TIMEOUT) {
                    Some(Event::Continued) => {}
                    other => panic!("the step's confirmation was expected, not {other:?}"),
                }
                let reason = expect_stop(remote);
                let statement = top_statement(remote, THREE_STATEMENTS);
                continue_stopped(remote);
                (reason, statement)
            },
        );
        assert_eq!(reason, StopReason::Step);
        assert_eq!(statement, "count:: count + 10");
        expect_ok(result, "run");
    }
}

/// A loop whose method stores its argument, then answers it by a return
/// that is a statement of its own.
const STORING: &str = indoc! {"
    class Storing = Object new (
      | last ::= 0. |
    ) (
      public run = (
        1 to: 3 do: [:index |
          store: index].
        ^last
      )
      store: value = (
        last:: value.
        ^value
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A stop decided at a slot whose breakpoints a `SetBreakpoints`
    /// served at the same safepoint removed is not made, and the frame
    /// fetches the slot again: the method breakpoint's exit standing there
    /// too, counted at the reach and not holding, is not counted again by
    /// that fetch. Its `Equal(4)` stops at the second call's exit, not at
    /// its entry, as a hit counted twice would. The `SetBreakpoints` is
    /// sent right after the `Continue` of the first statement's stop, long
    /// before the run reaches the return, and served at the safepoint the
    /// return's stop leaves for, as in
    /// `a_breakpoint_removed_before_its_stop_is_served_is_not_reported`;
    /// should it arrive later, the return's stop is made and reported, and
    /// is left out of what the test compares. Only the optimized lowering
    /// makes `^value` one `Return`, a statement start that is the exit;
    /// the reference lowering moves the value first, so there the exit
    /// stands at a slot of its own and the outcome is the same.
    fn a_stop_not_made_counts_no_hit_twice(lowering) {
        let first = line_start(STORING, "last:: value.");
        let returned = line_start(STORING, "^value");
        let (events, result) = debugged(
            STORING,
            lowering,
            move |remote| {
                set_breakpoints(remote, "Storing", vec![first, returned]);
                let mut breakpoint = method_breakpoint("Storing", &[], false, "store:");
                breakpoint.hit_condition = Some(HitCondition::Equal(4));
                set_method_breakpoints(remote, vec![breakpoint]);
            },
            move |remote| {
                let lines = expect_breakpoints(remote, "Storing");
                let method = only_method_breakpoint(&expect_method_breakpoints(remote), true);
                let reason = expect_stop(remote);
                remote.send(Command::Continue);
                set_breakpoints(remote, "Storing", vec![first]);
                (lines, method, reason, events_until_the_end(remote))
            },
        );
        expect_ok(result, "run");
        let (lines, method, reason, events) = events;
        assert_eq!(reason, StopReason::Breakpoint { ids: vec![lines[0].id] });
        let again = events
            .iter()
            .find_map(|event| match event {
                Event::Breakpoints { breakpoints, .. } => Some(breakpoints[0].id),
                _ => None,
            })
            .expect("the new set is answered");
        let reasons: Vec<StopReason> = events
            .into_iter()
            .filter_map(|event| match event {
                Event::Stopped { reason, .. } => Some(reason),
                _ => None,
            })
            .filter(|reason| *reason != StopReason::Breakpoint { ids: vec![lines[1].id] })
            .collect();
        assert_eq!(
            reasons,
            [
                StopReason::Breakpoint { ids: vec![again] },
                StopReason::MethodExit {
                    id: method,
                    others: Vec::new()
                },
                StopReason::Breakpoint { ids: vec![again] },
            ]
        );
    }
}

/// Every stop's reason, the run continued after each, until the run of
/// `source`'s `run` ends, with one method breakpoint on `selector` of the
/// module's root class whose hit condition is `condition`; and the
/// breakpoint's id.
fn method_stops(
    source: &str,
    module: &'static str,
    selector: &str,
    condition: HitCondition,
    lowering: Lowering,
) -> (BreakpointId, Vec<StopReason>) {
    let mut breakpoint = method_breakpoint(module, &[], false, selector);
    breakpoint.hit_condition = Some(condition);
    let (stops, result) = debugged(
        source,
        lowering,
        move |remote| set_method_breakpoints(remote, vec![breakpoint]),
        |remote| {
            let id = only_method_breakpoint(&expect_method_breakpoints(remote), true);
            let reasons = events_until_the_end(remote)
                .into_iter()
                .filter_map(|event| match event {
                    Event::Stopped { reason, .. } => Some(reason),
                    _ => None,
                })
                .collect();
            (id, reasons)
        },
    );
    expect_ok(result, "run");
    stops
}

/// The one stop a method breakpoint `id` makes at an exit.
fn only_exit(id: BreakpointId) -> [StopReason; 1] {
    [StopReason::MethodExit {
        id,
        others: Vec::new(),
    }]
}

ns_runtime::for_each_lowering! {
    /// A method of one statement, `^value`, counts its entry, then its
    /// exit: `Equal(2)` stops once, at the first call's exit. Under the
    /// optimized lowering its one instruction is both, the exit counted
    /// as the frame goes past the entry; under the reference lowering a
    /// move comes first.
    fn a_hit_condition_counts_an_entry_that_is_an_exit_twice(lowering) {
        let source = indoc! {"
            class Echoing = Object new (
              | last ::= 0. |
            ) (
              public run = (
                last:: echo: 1.
                last:: echo: 2.
                ^last
              )
              echo: value = (
                ^value
              )
            ) : ()
        "};
        let (id, stops) = method_stops(source, "Echoing", "echo:", HitCondition::Equal(2), lowering);
        assert_eq!(stops, only_exit(id));
    }
}

ns_runtime::for_each_lowering! {
    /// A method left by a non-local return of a block it passed on counts
    /// the exit once, where the return arrives: `Equal(4)` holds at the
    /// second call's exit only, the first call's entry and exit and the
    /// second's entry going past uncounted twice.
    fn a_hit_condition_counts_an_exit_by_a_non_local_return_once(lowering) {
        let source = indoc! {"
            class Finding = Object new (
              | last ::= 0. |
            ) (
              public run = (
                last:: find: 2.
                last:: last + (find: 3).
                ^last
              )
              find: target = (
                {1. 2. 3} do: [:each | each = target ifTrue: [^each]].
                ^0
              )
            ) : ()
        "};
        let (id, stops) = method_stops(source, "Finding", "find:", HitCondition::Equal(4), lowering);
        assert_eq!(stops, only_exit(id));
    }
}

ns_runtime::for_each_lowering! {
    /// A method's exit at a non-local return of a block lowered inline in
    /// it -- a block the reference lowering sends, whose return the
    /// breakpoint patches -- is counted at the return and not again where
    /// the return arrives: `Equal(4)` holds at the second call's exit only.
    fn a_hit_condition_counts_an_exit_at_an_inlined_non_local_return_once(lowering) {
        let source = indoc! {"
            class Picking = Object new (
              | last ::= 0. |
            ) (
              public run = (
                last:: pick: true.
                last:: last + (pick: true).
                ^last
              )
              pick: flag = (
                flag ifTrue: [^1].
                ^2
              )
            ) : ()
        "};
        let (id, stops) = method_stops(source, "Picking", "pick:", HitCondition::Equal(4), lowering);
        assert_eq!(stops, only_exit(id));
    }
}

ns_runtime::for_each_lowering! {
    /// A method an exception's unwind leaves counts the exit once:
    /// `Equal(4)` holds at the second call's exit only.
    fn a_hit_condition_counts_an_exit_by_an_exception_once(lowering) {
        let source = indoc! {"
            class Failing = Object new (
              | last ::= 0. |
            ) (
              public run = (
                last:: [fail] on: Error do: [:error | 1].
                last:: last + ([fail] on: Error do: [:error | 2]).
                ^last
              )
              fail = (
                Error signal: 'boom'.
                ^0
              )
            ) : ()
        "};
        let (id, stops) = method_stops(source, "Failing", "fail", HitCondition::Equal(4), lowering);
        assert_eq!(stops, only_exit(id));
    }
}

ns_runtime::for_each_lowering! {
    /// A lazy slot's getter, once its slot is filled, has its entry and its
    /// exit at its first instruction under both lowerings: the read counts
    /// the entry, then the exit as the frame goes past the entry. The first
    /// read counts its entry and the exit that computes the slot, the
    /// second its entry and, at the same slot, its exit, where `Equal(4)`
    /// holds.
    fn a_hit_condition_counts_a_filled_lazy_getter_s_entry_and_exit(lowering) {
        let source = indoc! {"
            class Lazing = Object new (
              | last ::= 0. |
            ) (
              lazy value = 21.
              public run = (
                last:: value + value.
                ^last
              )
            ) : ()
        "};
        let (id, stops) = method_stops(source, "Lazing", "value", HitCondition::Equal(4), lowering);
        assert_eq!(stops, only_exit(id));
    }
}

/// What `Command::ModuleText` answers for `module`.
fn module_text(
    remote: &DebugRemote,
    module: &str,
    known_revision: Option<u64>,
) -> (u64, ModuleTextAnswer) {
    let command = Command::ModuleText {
        module: String::from(module),
        known_revision,
    };
    match request(remote, command) {
        Event::ModuleText {
            module: answered,
            revision,
            text,
        } => {
            assert_eq!(answered, module);
            (revision, text)
        }
        other => panic!("the text of {module} was expected, not {other:?}"),
    }
}

#[test]
fn module_text_is_answered_once_per_revision() {
    let (first, known, none) = while_stopped(FOREVER, |remote| {
        (
            module_text(remote, "Forever", None),
            module_text(remote, "Forever", Some(0)),
            module_text(remote, "Forever", Some(7)),
        )
    });
    assert_eq!(first, (0, ModuleTextAnswer::Text(String::from(FOREVER))));
    assert_eq!(known, (0, ModuleTextAnswer::Unchanged));
    // A revision the agent could not have been told of is no match.
    assert_eq!(none, (0, ModuleTextAnswer::Text(String::from(FOREVER))));
}

#[test]
fn a_shipped_module_s_text_is_its_shipped_text() {
    let (revision, text) = while_stopped(FOREVER, |remote| module_text(remote, "Kernel", None));
    assert_eq!(revision, 0);
    let ModuleTextAnswer::Text(text) = text else {
        panic!("the kernel's text was expected, not {text:?}");
    };
    let first_line = "/// The kernel module: `Object` and the classes every program uses without";
    assert_eq!(text.lines().next(), Some(first_line));
}

#[test]
fn an_unknown_module_has_no_text() {
    let (revision, text) = while_stopped(FOREVER, |remote| module_text(remote, "Nowhere", None));
    assert_eq!((revision, text), (0, ModuleTextAnswer::None));
}

ns_runtime::for_each_lowering! {
    /// An install raises the revision, and the text a reader holds of an
    /// earlier one is stale: the module's current text is the edited one.
    fn an_install_raises_the_revision(lowering) {
        let (mut interpreter, class) = interpreted(EDITING, lowering);
        let platform = interpreter.platform();
        let instance = expect_ok(
            interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
            "usingPlatform:",
        );
        let offset = EDITING.find("value:: 6 * 7").expect("the first answer's statement");
        let offset = u32::try_from(offset).expect("a test's source is small");
        let ((before, after), result) = debugged_in(
            &mut interpreter,
            &instance,
            |remote| set_breakpoints(remote, "Editing", vec![offset]),
            |remote| {
                let (id, _) = resolved(&expect_breakpoints(remote, "Editing"));
                expect_breakpoints(remote, "Editing");
                assert_eq!(expect_breakpoint_stop(remote), [id]);
                let answers = (
                    module_text(remote, "Editing", None),
                    module_text(remote, "Editing", Some(0)),
                );
                continue_stopped(remote);
                answers
            },
        );
        assert_eq!(before.0, 1);
        assert_eq!(after.0, 1);
        let ModuleTextAnswer::Text(edited) = &before.1 else {
            panic!("the edited text was expected, not {:?}", before.1);
        };
        assert!(edited.contains("value:: 6 * 8"), "{edited}");
        assert_eq!(after.1, before.1);
        assert_eq!(answered(&mut interpreter, result), 48);
    }
}

/// A program that demands the mirrors module, built on first use, between
/// two statements.
const DEMANDING: &str = indoc! {"
    class Demanding usingPlatform: platform = Object new (
      | platform = platform. |
    ) (
      public run = (
        | before after |
        before:: 1.
        platform mirrors.
        after:: 2.
        ^after
      )
    ) : ()
"};

#[test]
fn a_shipped_module_not_built_yet_answers_its_shipped_text() {
    let (revision, text) = while_stopped(FOREVER, |remote| {
        module_text(remote, "CollectionsFactory", None)
    });
    assert_eq!(revision, 0);
    let ModuleTextAnswer::Text(text) = text else {
        panic!("the shipped text was expected, not {text:?}");
    };
    assert!(
        text.starts_with("/// The collections module: the growable Collections"),
        "{text}"
    );
}

#[test]
fn a_program_module_named_like_a_shipped_one_answers_its_own_text() {
    let source = indoc! {"
        class TimeFactory = Object new (
          | count ::= 0. |
        ) (
          public run = (
            [true] whileTrue: [count:: count + 1].
            ^count
          )
        ) : ()
    "};
    let expected = String::from(source);
    let (_, text) = while_stopped(source, |remote| module_text(remote, "TimeFactory", None));
    assert_eq!(text, ModuleTextAnswer::Text(expected));
}

ns_runtime::for_each_lowering! {
    /// Building a module raises the revision, so that a revision a text
    /// was read at is never the one of a later build.
    fn building_a_module_raises_the_revision(lowering) {
        let (mut interpreter, class) = interpreted(DEMANDING, lowering);
        let platform = interpreter.platform();
        let instance = expect_ok(
            interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
            "usingPlatform:",
        );
        let first = line_start(DEMANDING, "before:: 1");
        let second = line_start(DEMANDING, "after:: 2");
        let ((before, unchanged, after), result) = debugged_in(
            &mut interpreter,
            &instance,
            |remote| set_breakpoints(remote, "Demanding", vec![first, second]),
            |remote| {
                expect_breakpoints(remote, "Demanding");
                let first_stop = expect_breakpoint_stop(remote);
                assert_eq!(first_stop.len(), 1);
                let before = module_text(remote, "Demanding", None);
                let unchanged = module_text(remote, "Demanding", Some(before.0));
                continue_stopped(remote);
                let second_stop = expect_breakpoint_stop(remote);
                assert_eq!(second_stop.len(), 1);
                let after = module_text(remote, "Demanding", Some(before.0));
                continue_stopped(remote);
                (before, unchanged, after)
            },
        );
        assert_eq!(unchanged, (before.0, ModuleTextAnswer::Unchanged));
        assert_eq!(after.0, before.0 + 1);
        assert_eq!(after.1, before.1, "the same text, read at a later revision");
        assert_eq!(answered(&mut interpreter, result), 2);
    }
}

/// A module whose statements are of every kind a breakpoint stands at: a
/// superclass clause, a slot initializer, a method's statements, an
/// inlined block's and a closure's, the last two on one line.
const BREAKABLE: &str = indoc! {"
    class Breakable = Object new (
      | count ::= 0. |
    ) (
      public run = (
        count:: count + 1.
        count > 0 ifTrue: [count:: count + 2. count:: count + 3]. {4. 5} do: [:each | count:: count + each].
        ^count
      )
    ) : ()
"};

/// Each breakable statement of `source` under `lowering`, its text and
/// the text of the block it is the first statement of.
fn breakable_texts(source: &str, lowering: Lowering) -> Vec<(String, Option<String>)> {
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    interpreter
        .breakable_statements(&resolved_module(source))
        .into_iter()
        .map(|breakable| {
            (
                breakable.statement.text(source).to_string(),
                breakable.block.map(|block| block.text(source).to_string()),
            )
        })
        .collect()
}

#[test]
fn breakable_statements_name_every_statement_start() {
    let expected: Vec<(String, Option<String>)> = [
        ("Object new", None),
        ("count ::= 0.", None),
        ("count:: count + 1", None),
        (
            "count > 0 ifTrue: [count:: count + 2. count:: count + 3]",
            None,
        ),
        (
            "count:: count + 2",
            Some("[count:: count + 2. count:: count + 3]"),
        ),
        ("count:: count + 3", None),
        ("{4. 5} do: [:each | count:: count + each]", None),
        (
            "count:: count + each",
            Some("[:each | count:: count + each]"),
        ),
        ("^count", None),
    ]
    .into_iter()
    .map(|(statement, block)| (statement.to_string(), block.map(str::to_string)))
    .collect();
    let optimized = breakable_texts(BREAKABLE, Lowering::Optimized);
    assert_eq!(optimized, expected);
    assert_eq!(
        breakable_texts(BREAKABLE, Lowering::Reference),
        optimized,
        "both lowerings answer the same"
    );
}

ns_runtime::for_each_lowering! {
    /// Asking for a module's breakable statements leaves nothing behind:
    /// two calls answer alike, a breakpoint in the module asked about is
    /// pending until it is loaded, and loaded, the module runs to the
    /// answer it has in an interpreter never asked.
    fn breakable_statements_leave_the_interpreter_as_it_was(lowering) {
        let (mut interpreter, class) = interpreted(THREE_STATEMENTS, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let breakable = resolved_module(BREAKABLE);
        let first = interpreter.breakable_statements(&breakable);
        let second = interpreter.breakable_statements(&breakable);
        assert!(!first.is_empty());
        assert_eq!(first, second, "two calls answer alike");
        let offset = line_start(BREAKABLE, "count:: count + 1.");
        let (states, result) = debugged_in(
            &mut interpreter,
            &instance,
            |remote| {
                set_breakpoints(remote, "Breakable", vec![offset]);
                set_breakpoints(remote, "Breakable", Vec::new());
            },
            |remote| {
                let states = expect_breakpoints(remote, "Breakable");
                assert!(expect_breakpoints(remote, "Breakable").is_empty());
                states
            },
        );
        pending(&states);
        expect_ok(result, "run");
        let loaded = interpreter.load_module(&breakable);
        let loaded = expect_ok(interpreter.instantiate(&loaded, &[]), "instantiate");
        let after = run(&mut interpreter, &loaded);
        let (mut fresh, fresh_instance) = instance_of(BREAKABLE, lowering);
        let without = run(&mut fresh, &fresh_instance);
        assert_eq!(
            answered(&mut interpreter, after),
            answered(&mut fresh, without)
        );
    }
}
