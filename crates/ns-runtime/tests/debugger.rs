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
use ns_runtime::debugger::{BreakpointId, BreakpointState};
use ns_runtime::JitStatistics;
use ns_runtime::{
    Command, DebugRemote, Event, FrameDescription, HostResult, HostSignal, Interpreter, Lowering,
    MethodSide, Root, StopReason, Symbol, ValueDescription, ValueKind, Variable,
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
        expect_paused(&remote);
        continue_stopped(&remote);
        remote
    });
    let count = expect_ok(run(&mut interpreter, &instance), "the next run");
    drop(agent.join().expect("the remote's thread"));
    assert_eq!(small_integer(&mut interpreter, &count), 100_000);
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
        .as_ref()
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
    let children = while_stopped(STACKED, |remote| {
        let items = value_of(&loop_variables(remote), "items").id;
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
    let names: Vec<&str> = children.iter().map(|child| child.name.as_str()).collect();
    assert_eq!(names, ["field 0"]);
    assert!(
        shown(&children[0]).label.parse::<u64>().is_ok(),
        "the count slot holds a number, not {}",
        shown(&children[0]).label
    );
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
            Lowering::Optimized => assert_eq!(unused.value, None),
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
            Lowering::Optimized => assert_eq!(unused.value, None),
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
            Lowering::Optimized => assert_eq!(unused.value, None),
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

/// Sets the line breakpoints of `module` at `offsets`.
fn set_breakpoints(remote: &DebugRemote, module: &str, offsets: Vec<u32>) {
    remote.send(Command::SetBreakpoints {
        module: String::from(module),
        offsets,
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
/// three times.
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

/// `loop:` sums `Helper>>helper:` over its passes, a callee its optimized
/// copy inlines; `noop` does nothing.
const INLINING: &str = indoc! {"
    class Inlining = Object new () (
      class Helper = Object new () (
        public helper: value = (
          | doubled |
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

ns_runtime::for_each_lowering! {
    /// A breakpoint in a callee an optimized copy inlined frees the copy
    /// and stops at every call; no copy compiled while it stands inlines
    /// the callee.
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
        set_breakpoints(&remote, "Inlining", vec![line_start(INLINING, "^doubled + 1")]);
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
        let ((), result, _) = debugged_by(
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
        if pinned {
            assert_eq!(
                interpreter.jit_inlined_bodies(&class, MethodSide::Instance, selector),
                0,
                "no copy inlines the patched callee"
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
    let (mut interpreter, instance) = instance_of(source, lowering);
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
    /// patches, stops at the next statement, not where it stands.
    fn a_step_over_after_a_pause_at_a_push_stops_at_the_next_statement(lowering) {
        let (mut interpreter, instance) = instance_of(STEPPING, lowering);
        let remote = interpreter.attach_debugger();
        remote.pause();
        let ((before, after), result, _) = debugged_by(
            remote,
            |remote| {
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
