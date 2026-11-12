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
    Command, DebugRemote, Event, FrameDescription, HostResult, HostSignal, Interpreter, Lowering,
    Root, StopReason, Symbol, ValueDescription, ValueKind, Variable,
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
/// in its superseded body, which the module's text no longer holds.
const CHANGING: &str = indoc! {"
    class Changing usingPlatform: platform = Object new (
      | mirrors = platform mirrors. |
    ) (
      public class Subject = Object new () (
        public spin: count = (
          | items |
          items:: Array new: 2.
          change.
          [true] whileTrue: [].
          ^items
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

ns_runtime::for_each_lowering! {
    /// A frame running a body its module's text no longer compiles to has
    /// no metadata: it keeps its name, has no location and shows its
    /// arguments and registers by number (decision 5).
    fn a_changed_superseded_body_shows_unnamed_registers(lowering) {
        let (mut interpreter, class) = load(CHANGING, lowering);
        let platform = interpreter.platform();
        let instance = expect_ok(
            interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
            "usingPlatform:",
        );
        let ((frames, variables), _, _) =
            while_stopped_in(interpreter, &instance, PAUSE_DELAY, |remote| {
                frame_and_variables(remote, "Subject>>spin:")
            });
        let spinning = frame_named(&frames, "Subject>>spin:");
        assert_eq!(frames[spinning].location, None);
        assert_eq!(variables[0].name, "self");
        assert_eq!(value_of(&variables, "argument 1").label, "5");
        let items = variables
            .iter()
            .find(|variable| shown(variable).class_name == "Array")
            .expect("a register holds the Array");
        assert!(items.name.starts_with("register "), "{}", items.name);
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
