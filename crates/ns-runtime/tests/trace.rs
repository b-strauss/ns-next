//! The developer tool's trace (`docs/design/developer-tool.md`, "Stage 1:
//! tracing"): the sink a host installs, and what it hears. These pin
//! what the JIT's sites record, so they run with the JIT on whatever the
//! environment says, as `tests/jit.rs` does.
#![cfg(ns_jit)]

use std::cell::RefCell;
use std::rc::Rc;

use indoc::indoc;
use ns_runtime::trace_events::{CompileRefusal, CompileTrigger};
use ns_runtime::{
    HostResult, HostSignal, Interpreter, Root, SourceOrigin, Symbol, TraceCategories,
    TraceCategory, TraceEvent, TraceEventKind, TraceFilter, TraceSink,
};

/// A sink that keeps every event it receives where the test reads it.
struct Collector(Rc<RefCell<Vec<TraceEvent>>>);

impl TraceSink for Collector {
    fn event(&mut self, event: &TraceEvent) {
        self.0.borrow_mut().push(event.clone());
    }
}

/// A loop hot enough that the JIT compiles its method and switches into
/// it, and sends that fill caches.
const HOT: &str = indoc! {"
    class Hot = Object new () (
      public run = (
        | sum ::= 0. |
        1 to: 2000 do: [:index | sum:: sum + (self twice: index)].
        ^sum.
      )

      twice: number = (
        ^number * 2.
      )
    )
"};

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// `HOT` loaded and run, after `prepare` was given the interpreter;
/// answers the interpreter after the run.
fn run_hot(prepare: impl FnOnce(&mut Interpreter)) -> Interpreter {
    run_source(HOT, prepare)
}

/// `source`'s class loaded, instantiated and sent `run`, after `prepare`
/// was given the interpreter; answers the interpreter after the run.
fn run_source(source: &str, prepare: impl FnOnce(&mut Interpreter)) -> Interpreter {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(module, source.into(), SourceOrigin::File("test.ns".into()))
        .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    // On whatever the environment says: the sites that record are the
    // JIT's, and the stress gate runs the suite once with
    // `NS_NEXT_NO_JIT=1`.
    interpreter.set_jit_enabled(true);
    // Off whatever the environment says: a body the breakpoint or step
    // stress mode patched loses its machine code at every statement.
    #[cfg(debug_assertions)]
    interpreter.set_stress_breakpoints(false);
    #[cfg(debug_assertions)]
    interpreter.set_stress_step(false);
    prepare(&mut interpreter);
    let class = interpreter.load_module(&resolved);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("run"), &[]),
        "run",
    );
    interpreter
}

/// Tracing costs nothing it is not asked for: without a sink, and with a
/// sink for no category, a run that compiles records no event, so none
/// is delivered.
#[test]
fn no_sink_no_events_and_no_cost() {
    let mut without_sink = run_hot(|_| {});
    assert!(
        without_sink.jit_statistics().compiled_bodies > 0,
        "the run compiled"
    );
    assert_eq!(without_sink.trace_events_recorded(), 0);
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut without_categories = run_hot(|interpreter| {
        interpreter.set_trace(
            TraceCategories::default(),
            TraceFilter::everything(),
            Box::new(Collector(Rc::clone(&events))),
        );
    });
    assert!(without_categories.jit_statistics().compiled_bodies > 0);
    assert_eq!(without_categories.trace_events_recorded(), 0);
    assert!(events.borrow().is_empty());
}

/// The events of `categories` that `source`'s `run` delivers through
/// `filter`, after `prepare` was given the interpreter. The JIT and the
/// optimize stress modes are off whatever the environment says: the
/// first compiles every body at its first entry and abandons none, the
/// second optimizes every body at once, and these pin what the baseline
/// tier's thresholds decide.
fn traced(
    source: &str,
    categories: &[TraceCategory],
    filter: TraceFilter,
    prepare: impl FnOnce(&mut Interpreter),
) -> Vec<TraceEvent> {
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Box::new(Collector(Rc::clone(&events)));
    let categories = categories
        .iter()
        .fold(TraceCategories::default(), |set, category| {
            set.with(*category)
        });
    run_source(source, |interpreter| {
        interpreter.set_jit_stress(false);
        interpreter.set_optimize_stress(false);
        interpreter.set_optimization_budget(i64::MAX);
        interpreter.set_trace(categories, filter, sink);
        prepare(interpreter);
    });
    events.take()
}

/// The events about the body named `body`.
fn about<'events>(events: &'events [TraceEvent], body: &str) -> Vec<&'events TraceEvent> {
    events
        .iter()
        .filter(|event| event.body.as_deref() == Some(body))
        .collect()
}

/// A body is compiled at its second entry, an interpreted push: the event
/// names it as the debugger names its frame, says what triggered the
/// compile and how many bytes it took.
#[test]
fn a_body_is_traced_compiled_at_its_second_entry() {
    let source = indoc! {"
        class Test = Object new () (
          public run = (
            ^self twice + self twice.
          )

          twice = (
            | sum ::= 1. |
            sum:: sum + sum.
            ^sum.
          )
        )
    "};
    let events = traced(
        source,
        &[TraceCategory::Compile],
        TraceFilter::everything(),
        |_| {},
    );
    let compiled = about(&events, "Test>>twice")
        .into_iter()
        .find_map(|event| match event.kind {
            TraceEventKind::Compiled { trigger, bytes } => Some((event.category, trigger, bytes)),
            _ => None,
        });
    let Some((category, trigger, bytes)) = compiled else {
        panic!("no compile of `twice` traced: {events:?}");
    };
    assert_eq!(category, TraceCategory::Compile);
    assert_eq!(trigger, CompileTrigger::Entry);
    assert!(bytes > 0, "the body took bytes");
}

/// A loop sent once is compiled at a backward jump and its frame switched
/// into the machine code at the loop's head: both events name the same
/// head.
#[test]
fn a_loop_is_traced_entering_by_osr() {
    let source = indoc! {"
        class Test = Object new () (
          public run = (
            |
              sum ::= 0.
              index ::= 0.
            |
            [index < 100000] whileTrue: [
              sum:: sum + index.
              index:: index + 1.
            ].
            ^sum.
          )
        )
    "};
    let events = traced(
        source,
        &[TraceCategory::Compile, TraceCategory::Osr],
        TraceFilter::everything(),
        // The suspend stress mode serves a safepoint at the loop's
        // backward jumps, rebuilding the stack, and no jump reaches the
        // count that compiles.
        |interpreter| {
            #[cfg(debug_assertions)]
            interpreter.set_stress_suspend(false);
            #[cfg(not(debug_assertions))]
            let _ = interpreter;
        },
    );
    let run = about(&events, "Test>>run");
    let compiled = run.iter().find_map(|event| match event.kind {
        TraceEventKind::OsrCompiled { loop_head } => Some((event.category, loop_head)),
        _ => None,
    });
    let entered = run.iter().find_map(|event| match event.kind {
        TraceEventKind::OsrEntered { loop_head } => Some((event.category, loop_head)),
        _ => None,
    });
    let (Some((compiled_category, compiled_at)), Some((entered_category, entered_at))) =
        (compiled, entered)
    else {
        panic!("no on-stack replacement of `run` traced: {events:?}");
    };
    assert_eq!(compiled_category, TraceCategory::Osr);
    assert_eq!(entered_category, TraceCategory::Osr);
    assert_eq!(compiled_at, entered_at, "the loop's head");
    assert!(
        run.iter().any(|event| matches!(
            event.kind,
            TraceEventKind::Compiled {
                trigger: CompileTrigger::BackJump,
                ..
            }
        )),
        "the compile names its trigger: {events:?}"
    );
}

/// A body entered while the JIT is off is refused for good, and the event
/// says why.
#[test]
fn a_body_entered_while_the_jit_is_off_is_traced_refused() {
    let source = indoc! {"
        class Test = Object new () (
          public run = (
            ^3 + 4.
          )
        )
    "};
    let events = traced(
        source,
        &[TraceCategory::Compile],
        TraceFilter::everything(),
        |interpreter| interpreter.set_jit_enabled(false),
    );
    assert!(
        about(&events, "Test>>run").iter().any(|event| matches!(
            event.kind,
            TraceEventKind::CompileRefused {
                reason: CompileRefusal::JitOff,
                ..
            }
        )),
        "{events:?}"
    );
}

/// A body whose send its class answers in its own `doesNotUnderstand:`
/// leaves its machine code at every call, and is abandoned once it has
/// spent its exit budget, as in `tests/jit.rs`'s
/// `a_body_that_keeps_leaving_is_abandoned`.
#[test]
fn an_exiting_body_is_traced_abandoned() {
    let source = indoc! {"
        class Test = Object new () (
          twice: value = (
            ^value + value
          )
          step: value = (
            |
              doubled = self twice: value.
            |
            ^doubled + self one.
          )
          public doesNotUnderstand: message = (
            ^1
          )
          public run = (
            |
              sum ::= 0.
              index ::= 0.
            |
            [index < 1000] whileTrue: [
              sum:: sum + (self step: index).
              index:: index + 1.
            ].
            ^sum.
          )
        ) : ()
    "};
    let events = traced(
        source,
        &[TraceCategory::Compile],
        TraceFilter::everything(),
        // The zone stress mode frees `step:`'s code and compiles it
        // again, its exits counted from none, and the scavenge and
        // collect stress modes halve its exits at every collection they
        // force: under them a thousand calls need not spend its budget.
        |interpreter| {
            #[cfg(debug_assertions)]
            {
                interpreter.set_stress_zone(false);
                interpreter.set_stress_scavenge(false);
                interpreter.set_stress_collect(false);
            }
            #[cfg(not(debug_assertions))]
            let _ = interpreter;
        },
    );
    let abandoned = about(&events, "Test>>step:")
        .into_iter()
        .find_map(|event| match event.kind {
            TraceEventKind::Abandoned { exits } => Some((event.category, exits)),
            _ => None,
        });
    let Some((category, exits)) = abandoned else {
        panic!("no abandonment of `step:` traced: {events:?}");
    };
    assert_eq!(category, TraceCategory::Compile);
    assert!(exits > 0, "abandoned for its exits");
}

/// A filter of one method delivers that method's events only.
#[test]
fn the_filter_keeps_one_body() {
    let source = indoc! {"
        class Test = Object new () (
          public run = (
            |
              sum ::= 0.
              index ::= 0.
            |
            [index < 10] whileTrue: [
              sum:: sum + self a + self b.
              index:: index + 1.
            ].
            ^sum.
          )

          a = (
            | value ::= 1. |
            value:: value + 1.
            ^value.
          )

          b = (
            | value ::= 2. |
            value:: value + 1.
            ^value.
          )
        )
    "};
    let filter = TraceFilter::parse("Test>>a").expect("a valid filter");
    let events = traced(
        source,
        &[TraceCategory::Compile, TraceCategory::Osr],
        filter,
        |_| {},
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, TraceEventKind::Compiled { .. })),
        "`a` is compiled: {events:?}"
    );
    assert!(
        events
            .iter()
            .all(|event| event.body.as_deref() == Some("Test>>a")),
        "{events:?}"
    );
}

/// A callee reached only from machine code -- its sends start once the
/// loop runs in its compiled code -- is compiled at a compiled call's
/// miss: the event says so, recorded in the miss leaf and delivered after
/// it, at the next safepoint or the run's end.
#[test]
fn a_body_first_compiled_at_a_compiled_call_is_traced_call() {
    let source = indoc! {"
        class Test = Object new () (
          public run = (
            |
              sum ::= 0.
              index ::= 0.
            |
            [index < 1000] whileTrue: [
              index > 500 ifTrue: [sum:: sum + self callee].
              index:: index + 1.
            ].
            ^sum.
          )

          callee = (
            | value ::= 1. |
            value:: value + 1.
            ^value.
          )
        )
    "};
    let events = traced(
        source,
        &[TraceCategory::Compile],
        TraceFilter::everything(),
        // The suspend stress mode keeps the loop from being compiled, and
        // the pause stress mode's stops send the loop's frame back to the
        // interpreter, so that under either the callee's second entry
        // may be an interpreted push.
        |interpreter| {
            #[cfg(debug_assertions)]
            {
                interpreter.set_stress_suspend(false);
                interpreter.set_stress_pause(false);
            }
            #[cfg(not(debug_assertions))]
            let _ = interpreter;
        },
    );
    let callee = about(&events, "Test>>callee");
    assert!(
        callee.iter().any(|event| matches!(
            event.kind,
            TraceEventKind::Compiled {
                trigger: CompileTrigger::Call,
                ..
            }
        )),
        "{events:?}"
    );
    assert!(
        !callee.iter().any(|event| matches!(
            event.kind,
            TraceEventKind::Compiled {
                trigger: CompileTrigger::Entry,
                ..
            }
        )),
        "never at an interpreted entry: {events:?}"
    );
}
