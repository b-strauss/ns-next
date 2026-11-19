//! The developer tool's trace (`docs/design/developer-tool.md`, "Stage 1:
//! tracing"): the sink a host installs, and what it hears. These pin
//! what the JIT's sites record, so they run with the JIT on whatever the
//! environment says, as `tests/jit.rs` does.
#![cfg(ns_jit)]

use std::cell::RefCell;
use std::rc::Rc;

use indoc::{formatdoc, indoc};
use ns_runtime::trace_events::{
    CompactionFreeing, CompileRefusal, CompileTrigger, Deferral, FreeCause, InvalidationCause,
    RecompileWalkEnd, RetireReason,
};
use ns_runtime::{
    Command, DebugRemote, HostResult, HostSignal, Interpreter, LineBreakpoint, ModuleName, Root,
    SourceOrigin, Symbol, TraceCategories, TraceCategory, TraceEvent, TraceEventKind, TraceFilter,
    TraceSink,
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

/// How a test's class makes its instance: its plain factory, or
/// `usingPlatform:` with the interpreter's platform, for a program that
/// reaches the mirrors.
#[derive(Clone, Copy)]
enum Factory {
    Plain,
    UsingPlatform,
}

/// `source`'s class loaded, instantiated and sent `run`, after `prepare`
/// was given the interpreter; answers the interpreter after the run.
fn run_source(source: &str, prepare: impl FnOnce(&mut Interpreter)) -> Interpreter {
    run_source_made_by(source, Factory::Plain, prepare)
}

/// `run_source`, the instance made by `factory`.
fn run_source_made_by(
    source: &str,
    factory: Factory,
    prepare: impl FnOnce(&mut Interpreter),
) -> Interpreter {
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
    let instance = match factory {
        Factory::Plain => expect_ok(interpreter.instantiate(&class, &[]), "instantiate"),
        Factory::UsingPlatform => {
            let platform = interpreter.platform();
            expect_ok(
                interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
                "usingPlatform:",
            )
        }
    };
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
    traced_under(source, categories, filter, |interpreter| {
        interpreter.set_optimization_budget(i64::MAX);
        prepare(interpreter);
    })
}

/// `traced`, every body's optimization budget the JIT's own, as
/// `tests/jit.rs`'s `load_spending_with` leaves it: what the optimizing
/// tier's events need.
fn traced_spending(
    source: &str,
    categories: &[TraceCategory],
    prepare: impl FnOnce(&mut Interpreter),
) -> Vec<TraceEvent> {
    traced_under(source, categories, TraceFilter::everything(), prepare)
}

/// `traced_spending`, the instance made by `usingPlatform:`.
fn traced_spending_using_platform(
    source: &str,
    categories: &[TraceCategory],
    prepare: impl FnOnce(&mut Interpreter),
) -> Vec<TraceEvent> {
    traced_made_by(
        source,
        Factory::UsingPlatform,
        categories,
        TraceFilter::everything(),
        prepare,
    )
}

/// The events of `categories` that `source`'s `run` delivers through
/// `filter`, the JIT and optimize stress modes off, after `prepare` was
/// given the interpreter.
fn traced_under(
    source: &str,
    categories: &[TraceCategory],
    filter: TraceFilter,
    prepare: impl FnOnce(&mut Interpreter),
) -> Vec<TraceEvent> {
    traced_made_by(source, Factory::Plain, categories, filter, prepare)
}

/// `traced_under`, the instance made by `factory`.
fn traced_made_by(
    source: &str,
    factory: Factory,
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
    run_source_made_by(source, factory, |interpreter| {
        interpreter.set_jit_stress(false);
        interpreter.set_optimize_stress(false);
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

/// A loop whose method is hot: `run` sends `step:` a million times, from
/// its compiled loop, and its optimized copy inlines `step:`. A statement
/// no turn reaches stands before the loop, for a breakpoint that patches
/// the body without stopping.
const HOT_LOOP: &str = indoc! {"
    class Test = Object new () (
      public run = (
        |
          sum ::= 0.
          index ::= 0.
        |
        sum < 0 ifTrue: [
          ^0].
        [index < 1000000] whileTrue: [
          sum:: sum + (self step: index).
          index:: index + 1.
        ].
        ^sum.
      )

      step: value = (
        ^value + 1.
      )
    )
"};

/// Off whatever the environment says: the suspend stress mode serves a
/// safepoint at the loop's backward jumps, rebuilding the stack, so that
/// the loop is never compiled and its method spends no budget. The deopt
/// and invalidate stress modes went off with the optimize stress mode
/// (`traced_under`).
fn hot_loop_stress_off(interpreter: &mut Interpreter) {
    #[cfg(debug_assertions)]
    interpreter.set_stress_suspend(false);
    #[cfg(not(debug_assertions))]
    let _ = interpreter;
}

/// A hot loop's method is requested once its budget runs out -- the walk
/// up the stack saying where it stopped -- and installed, and its plan
/// inlines the callee its loop sends.
#[test]
fn a_hot_loop_is_traced_requested_and_optimized() {
    let events = traced_spending(
        HOT_LOOP,
        &[TraceCategory::Optimize, TraceCategory::Inlining],
        hot_loop_stress_off,
    );
    let run = about(&events, "Test>>run");
    assert!(
        run.iter()
            .any(|event| event.category == TraceCategory::Optimize
                && matches!(event.kind, TraceEventKind::OptimizationRequested { .. })),
        "{events:?}"
    );
    assert!(
        run.iter()
            .any(|event| event.category == TraceCategory::Optimize
                && matches!(event.kind, TraceEventKind::Optimized { bytes, .. } if bytes > 0)),
        "{events:?}"
    );
    assert!(
        run.iter()
            .any(|event| event.category == TraceCategory::Inlining
                && matches!(
                    &event.kind,
                    TraceEventKind::Inlined { in_body, callee, .. }
                        if in_body == "Test>>run" && callee == "Test>>step:"
                )),
        "{events:?}"
    );
}

/// A body a breakpoint patched stays in its baseline code: its request is
/// put off, and the event says why. The breakpoint stands on a statement
/// no turn reaches, so that the run never stops.
#[test]
fn a_patched_body_is_traced_deferred() {
    let mut remote: Option<DebugRemote> = None;
    let events = traced_spending(HOT_LOOP, &[TraceCategory::Optimize], |interpreter| {
        hot_loop_stress_off(interpreter);
        let attached = interpreter.attach_debugger();
        let offset = HOT_LOOP
            .find("^0].")
            .expect("the statement no turn reaches");
        attached.send(Command::SetBreakpoints {
            module: ModuleName::named("Test"),
            breakpoints: vec![LineBreakpoint::at_offset(
                u32::try_from(offset).expect("a small source"),
            )],
        });
        remote = Some(attached);
    });
    drop(remote);
    assert!(
        about(&events, "Test>>run").iter().any(|event| matches!(
            event.kind,
            TraceEventKind::OptimizationDeferred {
                reason: Deferral::Patched,
                ..
            }
        )),
        "{events:?}"
    );
}

/// Under the optimize stress mode a body's first spend requests its own
/// copy, with no walk up the stack: the request is traced as not walked,
/// so that every body optimized has its request.
#[test]
fn the_optimize_stress_mode_s_requests_are_traced_not_walked() {
    let events = traced_spending(HOT_LOOP, &[TraceCategory::Optimize], |interpreter| {
        hot_loop_stress_off(interpreter);
        interpreter.set_optimize_stress(true);
    });
    let optimized: Vec<&TraceEvent> = events
        .iter()
        .filter(|event| matches!(event.kind, TraceEventKind::Optimized { .. }))
        .collect();
    assert!(!optimized.is_empty(), "{events:?}");
    for event in optimized {
        assert!(
            events.iter().any(|request| request.body == event.body
                && matches!(
                    request.kind,
                    TraceEventKind::OptimizationRequested {
                        walk_end: RecompileWalkEnd::NotWalked,
                        ..
                    }
                )),
            "{events:?}"
        );
    }
}

/// `sum:` loops over a sum of its argument, optimized while every
/// argument is a `SmallInteger`; then given a `Float`, its copy's sum
/// traps, and traps again at the same site at the next call, which makes
/// the copy not entrant: retired for its traps.
#[test]
fn a_trapping_copy_is_traced_retired() {
    let source = indoc! {"
        class Test = Object new () (
          public run = (
            |
              total ::= 0.
              index ::= 0.
            |
            [index < 2000] whileTrue: [
              total:: total + (self sum: 1).
              index:: index + 1.
            ].
            index:: 0.
            [index < 20] whileTrue: [
              total:: total + (self sum: 0.5) asInteger.
              index:: index + 1.
            ].
            ^total.
          )

          sum: value = (
            |
              sum ::= 0.
              index ::= 0.
            |
            [index < 1000] whileTrue: [
              sum:: sum + value.
              index:: index + 1.
            ].
            ^sum.
          )
        )
    "};
    let events = traced_spending(source, &[TraceCategory::Deoptimize], |interpreter| {
        hot_loop_stress_off(interpreter);
        // The zone stress mode's compactions free the copy between its
        // traps, so that no copy traps twice.
        #[cfg(debug_assertions)]
        interpreter.set_stress_zone(false);
    });
    assert!(
        events
            .iter()
            .any(|event| event.category == TraceCategory::Deoptimize
                && matches!(
                    event.kind,
                    TraceEventKind::Retired {
                        reason: RetireReason::Traps,
                        ..
                    }
                )),
        "{events:?}"
    );
}

/// `run`'s copy inlines `Counter>>step:`, its type-case guarded on
/// `Counter`'s application; the edit that replaces `step:` rebuilds that
/// application, and frees the copy for it.
#[test]
fn an_edit_is_traced_invalidating_the_copy() {
    let source = indoc! {"
        class Test usingPlatform: platform = Object new (
          |
            ClassMirror = platform mirrors ClassMirror.
          |
        ) (
          class Counter = Object new () (
            public step: value = (
              ^value + 1.
            )
          )

          public run = (
            |
              sum ::= 0.
              index ::= 0.
              counter = Counter new.
            |
            [index < 1000000] whileTrue: [
              sum:: sum + (counter step: index).
              index:: index + 1.
            ].
            self edit.
            ^sum.
          )

          edit = (
            | builder |
            builder:: (ClassMirror reflecting: Counter) mixin declaration asBuilder.
            builder instanceSide methods addFromSource: 'public step: value = ( ^value + 2 )'.
            builder install.
          )
        )
    "};
    let events =
        traced_spending_using_platform(source, &[TraceCategory::Deoptimize], hot_loop_stress_off);
    assert!(
        about(&events, "Test>>run").iter().any(|event| matches!(
            event.kind,
            TraceEventKind::Invalidated {
                cause: InvalidationCause::Edit,
                ..
            }
        )),
        "{events:?}"
    );
}

/// The program of `tests/jit.rs`'s
/// `a_zone_with_room_for_a_few_bodies_leaves_the_output_right`: more
/// bodies than a zone with room for a few holds, each compiled at its
/// first entry, so that the zone is compacted, freeing bodies.
#[test]
fn a_compaction_is_traced() {
    let count: i64 = 300;
    let methods: String = (0..count)
        .map(|index| format!("  m{index} = (\n    ^{index}\n  )\n"))
        .collect();
    let sends: String = (0..count)
        .map(|index| format!("    sum:: sum + self m{index} + self m{index}.\n"))
        .collect();
    let source = formatdoc! {"
        class Test = Object new () (
        {methods}  public run = (
            | sum |
            sum:: 0.
        {sends}    ^sum
          )
        ) : ()
    "};
    let events = traced(
        &source,
        &[TraceCategory::CodeZone],
        TraceFilter::everything(),
        |interpreter| {
            // The room past the stubs, not the zone's size, is what the
            // test fixes, since the stubs' size is the architecture's.
            interpreter.set_jit_zone_bytes(1 << 20);
            let stubs = interpreter.table_sizes().zone_used_bytes;
            interpreter.set_jit_zone_bytes(stubs + 1_344);
            interpreter.set_jit_stress(true);
        },
    );
    assert!(
        events.iter().any(|event| event.category == TraceCategory::CodeZone
            && event.body.is_none()
            && matches!(
                event.kind,
                TraceEventKind::Compacted {
                    freeing: CompactionFreeing::ToAQuarter | CompactionFreeing::EveryInactiveBody,
                    bodies_freed,
                } if bodies_freed > 0
            )),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.category == TraceCategory::CodeZone
                && event.body.is_some()
                && matches!(
                    event.kind,
                    TraceEventKind::Freed {
                        cause: FreeCause::Compaction,
                        ..
                    }
                )),
        "{events:?}"
    );
}
