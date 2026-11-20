//! The developer tool's trace (`docs/design/developer-tool.md`, "Stage 1:
//! tracing"): the sink a host installs, and what it hears. These pin
//! what the JIT's sites record, so they run with the JIT on whatever the
//! environment says, as `tests/jit.rs` does.
#![cfg(ns_jit)]

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use indoc::{formatdoc, indoc};
use ns_runtime::trace_events::{
    CodeTier, CompactionFreeing, CompileRefusal, CompileTrigger, Deferral, FreeCause, HoldReason,
    InvalidationCause, OsrRefusal, RecompileWalkEnd, RetireReason, SiteState, VacateCause,
};
use ns_runtime::{
    Command, DebugRemote, HostResult, HostSignal, Interpreter, LineBreakpoint, ModuleName,
    Printing, Root, SourceOrigin, Symbol, TraceCategories, TraceCategory, TraceEvent,
    TraceEventKind, TraceFilter, TraceSettings, TraceSink,
};

/// A sink that keeps every event it receives where the test reads it.
struct Collector(Rc<RefCell<Vec<TraceEvent>>>);

impl TraceSink for Collector {
    fn event(&mut self, event: &TraceEvent) {
        self.0.borrow_mut().push(event.clone());
    }
}

/// The settings that trace `categories` through `filter` and print
/// nothing: stage 1's.
fn tracing(categories: TraceCategories, filter: TraceFilter) -> TraceSettings {
    TraceSettings {
        categories,
        filter,
        printing: Printing::default(),
        print_filter: TraceFilter::everything(),
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
    run_source_made_by(source, Factory::Plain, prepare, |_| false)
}

/// `run_source`, the instance made by `factory`, and sent `run` once more
/// when `again`, given the interpreter after the first run, says so.
fn run_source_made_by(
    source: &str,
    factory: Factory,
    prepare: impl FnOnce(&mut Interpreter),
    again: impl FnOnce(&mut Interpreter) -> bool,
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
    if again(&mut interpreter) {
        expect_ok(
            interpreter.send(&instance, Symbol::intern("run"), &[]),
            "run again",
        );
    }
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
            tracing(TraceCategories::default(), TraceFilter::everything()),
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
        |_| false,
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
    traced_made_by(source, Factory::Plain, categories, filter, prepare, |_| {
        false
    })
}

/// `traced_under`, the instance made by `factory`, and sent `run` once
/// more when `again` says so (`run_source_made_by`).
fn traced_made_by(
    source: &str,
    factory: Factory,
    categories: &[TraceCategory],
    filter: TraceFilter,
    prepare: impl FnOnce(&mut Interpreter),
    again: impl FnOnce(&mut Interpreter) -> bool,
) -> Vec<TraceEvent> {
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Box::new(Collector(Rc::clone(&events)));
    let categories = categories
        .iter()
        .fold(TraceCategories::default(), |set, category| {
            set.with(*category)
        });
    run_source_made_by(
        source,
        factory,
        |interpreter| {
            interpreter.set_jit_stress(false);
            interpreter.set_optimize_stress(false);
            interpreter.set_trace(tracing(categories, filter), sink);
            prepare(interpreter);
        },
        again,
    );
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

/// `step:` sends `one`, which its class answers in its own
/// `doesNotUnderstand:`: its machine code leaves at every call.
const ABANDONING: &str = indoc! {"
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

/// Off whatever the environment says: the zone stress mode frees
/// `ABANDONING`'s `step:` and compiles it again, its exits counted from
/// none, and the scavenge and collect stress modes halve its exits at
/// every collection they force: under them a thousand calls need not
/// spend its budget.
fn abandoning_stress_off(interpreter: &mut Interpreter) {
    #[cfg(debug_assertions)]
    {
        interpreter.set_stress_zone(false);
        interpreter.set_stress_scavenge(false);
        interpreter.set_stress_collect(false);
    }
    #[cfg(not(debug_assertions))]
    let _ = interpreter;
}

/// A body whose send its class answers in its own `doesNotUnderstand:`
/// leaves its machine code at every call, and is abandoned once it has
/// spent its exit budget, as in `tests/jit.rs`'s
/// `a_body_that_keeps_leaving_is_abandoned`.
#[test]
fn an_exiting_body_is_traced_abandoned() {
    let events = traced(
        ABANDONING,
        &[TraceCategory::Compile],
        TraceFilter::everything(),
        abandoning_stress_off,
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
    let filter = TraceFilter::parse(&["Test>>a"]).expect("a valid filter");
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
/// the copy not entrant.
const TRAPPING: &str = indoc! {"
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

/// Off whatever the environment says, beside `hot_loop_stress_off`'s: the
/// zone stress mode's compactions free `TRAPPING`'s copy between its
/// traps, so that no copy traps twice.
fn trapping_stress_off(interpreter: &mut Interpreter) {
    hot_loop_stress_off(interpreter);
    #[cfg(debug_assertions)]
    interpreter.set_stress_zone(false);
}

/// `TRAPPING`'s copy is retired for its traps.
#[test]
fn a_trapping_copy_is_traced_retired() {
    let events = traced_spending(TRAPPING, &[TraceCategory::Deoptimize], trapping_stress_off);
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

/// A retired copy's free is its `Retired` event alone: with both
/// categories on, no `Freed` event of optimized code stands beside it.
/// Nothing else frees a copy of `TRAPPING`'s once the collect stress mode,
/// whose collections free copies by their feedback bases, is off too.
#[test]
fn a_retired_copy_is_one_event() {
    let events = traced_spending(
        TRAPPING,
        &[TraceCategory::Deoptimize, TraceCategory::CodeZone],
        |interpreter| {
            trapping_stress_off(interpreter);
            #[cfg(debug_assertions)]
            interpreter.set_stress_collect(false);
        },
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, TraceEventKind::Retired { .. })),
        "{events:?}"
    );
    assert!(
        !events.iter().any(|event| matches!(
            event.kind,
            TraceEventKind::Freed {
                tier: CodeTier::Optimized,
                ..
            }
        )),
        "{events:?}"
    );
}

/// `step:at:` sends `probe:` to a `Stranger`, which answers it in its own
/// `doesNotUnderstand:`, whose handler machine code leaves to the
/// interpreter: its optimized code exits at every call, spends its exit
/// budget and is retired for its exits, as in `tests/jit.rs`'s
/// `a_copy_abandoned_past_its_last_version_stays_in_baseline_code`.
#[test]
fn an_exiting_copy_is_traced_retired_for_its_exits() {
    let source = indoc! {"
        class Test = Object new (
          | stranger = Stranger new. |
        ) (
          class Stranger = () (
            public doesNotUnderstand: message = (
              ^1
            )
          )

          step: target at: value = (
            ^target probe: value
          )

          public run = (
            |
              sum ::= 0.
              index ::= 0.
            |
            [index < 40000] whileTrue: [
              sum:: sum + (self step: stranger at: index).
              index:: index + 1.
            ].
            ^sum
          )
        ) : ()
    "};
    let events = traced_spending(source, &[TraceCategory::Deoptimize], |interpreter| {
        hot_loop_stress_off(interpreter);
        interpreter.set_optimization_budget(100);
        interpreter.set_jit_revival_entries(64);
        // The zone stress mode frees the copy and compiles it again, its
        // exits counted from none, and the scavenge and collect stress
        // modes halve its exits at every collection they force: under
        // them the copy need not spend its budget; nor under the cache
        // stress mode, which holds the safepoint request and vacates the
        // caches at every poll, changing which copy runs when.
        #[cfg(debug_assertions)]
        {
            interpreter.set_stress_zone(false);
            interpreter.set_stress_scavenge(false);
            interpreter.set_stress_collect(false);
            interpreter.set_stress_caches(false);
        }
    });
    assert!(
        about(&events, "Test>>step:at:")
            .iter()
            .any(|event| matches!(
                event.kind,
                TraceEventKind::Retired {
                    reason: RetireReason::Exits,
                    ..
                }
            )),
        "{events:?}"
    );
}

/// A breakpoint set on a compiled body frees its machine code: `run`,
/// compiled in the first run, is patched between the runs, on a statement
/// no turn reaches, and its code is freed at the second run's first
/// safepoint.
#[test]
fn a_breakpoint_on_a_compiled_body_is_traced_freeing_it() {
    let mut remote: Option<DebugRemote> = None;
    let events = traced_made_by(
        HOT_LOOP,
        Factory::Plain,
        &[TraceCategory::CodeZone],
        TraceFilter::everything(),
        |interpreter| {
            hot_loop_stress_off(interpreter);
            interpreter.set_optimization_budget(i64::MAX);
        },
        |interpreter| {
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
            true
        },
    );
    drop(remote);
    assert!(
        about(&events, "Test>>run").iter().any(|event| matches!(
            event.kind,
            TraceEventKind::Freed {
                key: None,
                tier: CodeTier::Baseline,
                cause: FreeCause::Breakpoint,
                ..
            }
        )),
        "{events:?}"
    );
}

/// `Counter>>step:`, compiled at its second entry, is replaced by an edit;
/// nothing runs its old body any more, and the full collection after the
/// edit frees its machine code.
#[test]
fn a_dead_body_is_traced_freed_by_a_full_collection() {
    let source = indoc! {"
        class Test usingPlatform: platform = Object new (
          |
            ClassMirror = platform mirrors ClassMirror.
            kernel = platform kernel.
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
              counter = Counter new.
            |
            sum:: sum + (counter step: 1).
            sum:: sum + (counter step: 2).
            sum:: sum + (counter step: 3).
            self edit.
            kernel garbageCollect.
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
    let events = traced_made_by(
        source,
        Factory::UsingPlatform,
        &[TraceCategory::CodeZone],
        TraceFilter::everything(),
        |interpreter| {
            interpreter.set_optimization_budget(i64::MAX);
            // The zone stress mode's compactions free the old body before
            // the collection does.
            #[cfg(debug_assertions)]
            interpreter.set_stress_zone(false);
            #[cfg(not(debug_assertions))]
            let _ = interpreter;
        },
        |_| false,
    );
    assert!(
        events.iter().any(|event| matches!(
            event.kind,
            TraceEventKind::Freed {
                cause: FreeCause::DeadBody,
                ..
            }
        )),
        "{events:?}"
    );
}

/// Setting the zone's size after a run replaces it, dropping every
/// body's machine code at once: one event, naming no body, counts them,
/// delivered before the setter returns, with no run after it.
#[test]
fn a_zone_replaced_is_traced() {
    let mut replaced = None;
    let events = traced_made_by(
        HOT,
        Factory::Plain,
        &[TraceCategory::CodeZone],
        TraceFilter::everything(),
        // The zone stress mode's compactions free every body no
        // activation runs, so that none is left for the replacement.
        |interpreter| {
            #[cfg(debug_assertions)]
            interpreter.set_stress_zone(false);
            #[cfg(not(debug_assertions))]
            let _ = interpreter;
        },
        |interpreter| {
            replaced = Some(interpreter.jit_statistics().compiled_bodies);
            interpreter.set_jit_zone_bytes(1 << 20);
            // Delivered before the hook returns, with no run after it.
            false
        },
    );
    assert!(
        replaced.is_some_and(|compiled| compiled > 0),
        "the run compiled"
    );
    assert!(
        events.iter().any(|event| event.body.is_none()
            && matches!(
                event.kind,
                TraceEventKind::ZoneReplaced { bodies_freed } if bodies_freed > 0
            )),
        "{events:?}"
    );
}

/// Six receivers of six classes sent `value` at one site, `send:`'s
/// only one.
const SIX_RECEIVERS: &str = indoc! {"
    class Test = Object new () (
      class First = Object new () (
        public value = (
          ^1.
        )
      )

      class Second = Object new () (
        public value = (
          ^2.
        )
      )

      class Third = Object new () (
        public value = (
          ^3.
        )
      )

      class Fourth = Object new () (
        public value = (
          ^4.
        )
      )

      class Fifth = Object new () (
        public value = (
          ^5.
        )
      )

      public run = (
        | sum ::= 0. |
        sum:: sum + (self send: [0]).
        sum:: sum + (self send: First new).
        sum:: sum + (self send: Second new).
        sum:: sum + (self send: Third new).
        sum:: sum + (self send: Fourth new).
        sum:: sum + (self send: Fifth new).
        ^sum.
      )

      send: object = (
        ^object value.
      )
    )
"};

/// The cache stress mode vacates every site at every safepoint, so that
/// a site starts afresh where these pin how it moves on.
fn caches_stress_off(interpreter: &mut Interpreter) {
    #[cfg(debug_assertions)]
    interpreter.set_stress_caches(false);
    #[cfg(not(debug_assertions))]
    let _ = interpreter;
}

/// A site is traced through its states as its receivers' classes grow:
/// monomorphic at the first, polymorphic at the second, megamorphic at
/// the fifth, each transition naming the send's body, selector, site and
/// receiver, its class qualified as every class is.
#[test]
fn a_site_is_traced_through_its_states() {
    let events = traced(
        SIX_RECEIVERS,
        &[TraceCategory::Caches],
        TraceFilter::everything(),
        caches_stress_off,
    );
    let transitions: Vec<(SiteState, SiteState, String, String, u32)> =
        about(&events, "Test>>send:")
            .into_iter()
            .filter_map(|event| match &event.kind {
                TraceEventKind::CacheTransition {
                    before,
                    after,
                    selector,
                    site,
                    receiver,
                } => {
                    assert_eq!(event.category, TraceCategory::Caches);
                    Some((*before, *after, selector.clone(), receiver.clone(), *site))
                }
                _ => None,
            })
            .collect();
    let expected = [
        (
            SiteState::Vacant,
            SiteState::Monomorphic,
            "newspeak.core.Kernel/Closure",
        ),
        (SiteState::Monomorphic, SiteState::Polymorphic, "Test/First"),
        (
            SiteState::Polymorphic,
            SiteState::Megamorphic,
            "Test/Fourth",
        ),
    ];
    let mut remaining = transitions.iter();
    for (before, after, receiver) in expected {
        assert!(
            remaining.any(|transition| transition.0 == before
                && transition.1 == after
                && transition.2 == "value"
                && transition.3 == receiver
                && transition.4 == 0),
            "no {}->{} for {receiver} in order: {transitions:?}",
            before.name(),
            after.name()
        );
    }
    // A megamorphic site stays so: the sixth receiver records nothing.
    let at_the_site = |state: SiteState, position: fn(&(SiteState, SiteState)) -> SiteState| {
        transitions
            .iter()
            .filter(|transition| {
                transition.4 == 0 && position(&(transition.0, transition.1)) == state
            })
            .count()
    };
    assert_eq!(
        at_the_site(SiteState::Megamorphic, |states| states.1),
        1,
        "{transitions:?}"
    );
    assert_eq!(
        at_the_site(SiteState::Megamorphic, |states| states.0),
        0,
        "{transitions:?}"
    );
}

/// A site that misses only once its body runs machine code moves on in
/// the miss leaf, which records the transition and leaves it queued; it
/// is delivered at the next safepoint or the host's return, after the
/// body's compile.
#[test]
fn a_transition_the_miss_leaf_makes_is_traced() {
    let source = indoc! {"
        class Test = Object new () (
          class First = Object new () (
            public value = (
              ^1.
            )
          )

          class Second = Object new () (
            public value = (
              ^2.
            )
          )

          public run = (
            |
              sum ::= 0.
              first = First new.
            |
            1 to: 2000 do: [:index | sum:: sum + (self send: first)].
            ^sum + (self send: Second new).
          )

          send: object = (
            ^object value.
          )
        )
    "};
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Box::new(Collector(Rc::clone(&events)));
    let interpreter = run_source(source, |interpreter| {
        interpreter.set_jit_stress(false);
        interpreter.set_optimize_stress(false);
        interpreter.set_optimization_budget(i64::MAX);
        caches_stress_off(interpreter);
        interpreter.set_trace(
            tracing(
                TraceCategories::default()
                    .with(TraceCategory::Compile)
                    .with(TraceCategory::Caches),
                TraceFilter::everything(),
            ),
            sink,
        );
    });
    assert!(
        interpreter.jit_statistics().miss_leaf_calls > 0,
        "machine code's sends missed"
    );
    let events = events.take();
    let send = about(&events, "Test>>send:");
    let compiled = send
        .iter()
        .position(|event| matches!(event.kind, TraceEventKind::Compiled { .. }));
    let polymorphic = send.iter().position(|event| {
        matches!(
            &event.kind,
            TraceEventKind::CacheTransition {
                before: SiteState::Monomorphic,
                after: SiteState::Polymorphic,
                receiver,
                ..
            } if receiver == "Test/Second"
        )
    });
    assert!(
        matches!((compiled, polymorphic), (Some(compiled), Some(polymorphic)) if compiled < polymorphic),
        "the site moved on after its body's compile: {send:?}"
    );
}

/// Polymorphic sends record nothing once their site settled, and what
/// was recorded is delivered when the run returns: the queue is empty.
#[test]
fn the_queue_is_empty_after_a_run() {
    let source = indoc! {"
        class Test = Object new () (
          class Other = Object new () (
            public value = (
              ^2.
            )
          )

          public run = (
            |
              sum ::= 0.
              block = [1].
              other = Other new.
            |
            1 to: 50000 do: [:index |
              sum:: sum + (self send: block) + (self send: other).
            ].
            ^sum.
          )

          send: object = (
            ^object value.
          )
        )
    "};
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Box::new(Collector(Rc::clone(&events)));
    let mut interpreter = run_source(source, |interpreter| {
        interpreter.set_jit_stress(false);
        interpreter.set_optimize_stress(false);
        interpreter.set_optimization_budget(i64::MAX);
        caches_stress_off(interpreter);
        interpreter.set_trace(
            tracing(
                TraceCategories::default().with(TraceCategory::Caches),
                TraceFilter::everything(),
            ),
            sink,
        );
    });
    assert!(
        events.borrow().iter().any(|event| matches!(
            event.kind,
            TraceEventKind::CacheTransition {
                after: SiteState::Polymorphic,
                ..
            }
        )),
        "{:?}",
        events.borrow()
    );
    assert_eq!(interpreter.trace_queue_length(), 0);
}

/// `tests/jit.rs`'s program of the same name: `Cell`, applied once over
/// `Narrow` and once over `Wide`, its lazy getter `step` customized per
/// receiver application under the optimize stress mode; and `collect`,
/// the program's own full collection, which a build without debug
/// assertions has no host's for.
const ONE_MIXIN_OVER_TWO_BASES: &str = indoc! {"
    class Subject usingPlatform: platform = Object new (
      |
        kernel = platform kernel.
      |
    ) (
      class Narrow = Object new () ()

      class Wide = Object new (
        |
          public first ::= 100.
        |
      ) ()

      class Box on: base = Object new (
        |
          base = base.
        |
      ) (
        public class Cell = base new () (
          lazy public step = 2.
        )
      )

      public bases = (
        ^Narrow == Wide ifTrue: [1] ifFalse: [0].
      )

      public narrow = (
        ^(Box on: Narrow) Cell new.
      )

      public sumOf: cell = (
        |
          sum ::= 0.
          index ::= 0.
        |
        [index < 100] whileTrue: [
          sum:: sum + cell step.
          index:: index + 1.
        ].
        ^sum.
      )

      public collect = (
        kernel garbageCollect.
      )
    ) : ()
"};

/// A full collection that reclaims `Cell` over `Narrow`'s application
/// frees the copies keyed on its dispatch id, and the invalidation names
/// the key's class as it was before the id was freed -- never `#<id>`,
/// which the flush after the collection would find for an id no
/// application holds.
#[test]
fn a_reclaimed_id_s_invalidation_names_its_class() {
    let module = ns_syntax::parse(ONE_MIXIN_OVER_TWO_BASES).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        ONE_MIXIN_OVER_TWO_BASES.into(),
        SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut interpreter = Interpreter::bootstrap();
    interpreter.set_jit_enabled(true);
    interpreter.set_jit_stress(true);
    interpreter.set_optimize_stress(true);
    // Each would free the copies, or reclaim the id, before the
    // collection this test makes; a build without debug assertions has
    // none of them.
    #[cfg(debug_assertions)]
    {
        interpreter.set_stress_breakpoints(false);
        interpreter.set_stress_step(false);
        interpreter.set_stress_zone(false);
        interpreter.set_stress_collect(false);
        interpreter.set_stress_suspend(false);
    }
    interpreter.set_trace(
        tracing(
            TraceCategories::default().with(TraceCategory::Deoptimize),
            TraceFilter::everything(),
        ),
        Box::new(Collector(Rc::clone(&events))),
    );
    let class = interpreter.load_module(&resolved);
    let platform = interpreter.platform();
    let subject = expect_ok(
        interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
        "usingPlatform:",
    );
    expect_ok(
        interpreter.send(&subject, Symbol::intern("bases"), &[]),
        "bases",
    );
    for _ in 0..3 {
        let cell = expect_ok(
            interpreter.send(&subject, Symbol::intern("narrow"), &[]),
            "narrow",
        );
        expect_ok(
            interpreter.send(&subject, Symbol::intern("sumOf:"), &[cell]),
            "sumOf:",
        );
    }
    // Served at the safepoint of `collect`'s return.
    expect_ok(
        interpreter.send(&subject, Symbol::intern("collect"), &[]),
        "collect",
    );
    // A run's return delivers what the collection recorded.
    expect_ok(
        interpreter.send(&subject, Symbol::intern("bases"), &[]),
        "bases",
    );
    let events = events.take();
    let reclaimed: Vec<&TraceEvent> = events
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                TraceEventKind::Invalidated {
                    cause: InvalidationCause::ReclaimedId,
                    ..
                }
            )
        })
        .collect();
    assert!(
        reclaimed.iter().any(|event| matches!(
            &event.kind,
            TraceEventKind::Invalidated { key: Some(key), .. } if key == "Subject/Box.Cell"
        )),
        "{events:?}"
    );
    for event in reclaimed {
        let TraceEventKind::Invalidated { key, entered, .. } = &event.kind else {
            unreachable!("filtered to invalidations");
        };
        for name in key.iter().chain(entered) {
            assert!(!name.starts_with('#'), "{events:?}");
        }
    }
}

/// The JIT switched off after `ABANDONING`'s `step:` was abandoned refuses
/// it for good, which its abandonment's revival would have compiled
/// again: traced as such. The switch, and the host's own request after
/// it, vacate every cache, each traced once, about no body.
#[test]
fn switching_the_jit_off_is_traced_refusing_the_abandoned_and_vacating_the_caches() {
    let events = traced_made_by(
        ABANDONING,
        Factory::Plain,
        &[TraceCategory::Compile, TraceCategory::Caches],
        TraceFilter::everything(),
        |interpreter| {
            abandoning_stress_off(interpreter);
            // No revival by entries before the switch.
            interpreter.set_jit_revival_entries(u32::MAX);
        },
        |interpreter| {
            // Each delivers its events before it returns, with no run
            // after them.
            interpreter.set_jit_enabled(false);
            interpreter.invalidate_caches();
            false
        },
    );
    let step = about(&events, "Test>>step:");
    assert!(
        step.iter()
            .any(|event| matches!(event.kind, TraceEventKind::Abandoned { .. })),
        "{events:?}"
    );
    assert!(
        step.iter()
            .any(|event| event.category == TraceCategory::Compile
                && matches!(
                    event.kind,
                    TraceEventKind::AbandonedRefused {
                        reason: CompileRefusal::JitOff
                    }
                )),
        "{events:?}"
    );
    for cause in [VacateCause::JitSwitched, VacateCause::Host] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event.category == TraceCategory::Caches
                    && event.body.is_none()
                    && event.kind == TraceEventKind::CachesVacated { cause })
                .count(),
            1,
            "{}: {events:?}",
            cause.name()
        );
    }
}

/// A recursion through more bodies than a small zone holds, as in
/// `tests/jit.rs`'s `a_zone_full_of_active_code_does_not_compact_at_every_safepoint`,
/// fills the zone with active code: a body that comes due while the zone
/// stays exhausted is held rather than tried, traced once until it is
/// compiled -- not at every entry the exhausted zone holds it at.
#[test]
fn a_compile_the_exhausted_zone_holds_is_traced_once() {
    let count = 300;
    let methods: String = (0..count)
        .map(|index| {
            let next = if index + 1 == count {
                "0".to_string()
            } else {
                format!("(self m{}) + 1", index + 1)
            };
            formatdoc! {"
                  m{index} = (
                    ^{next}
                  )
            "}
        })
        .collect();
    let source = formatdoc! {"
        class Test = Object new () (
        {methods}  public run = (
            ^(self m0) + (self m0) + (self m0)
          )
        ) : ()
    "};
    let events = traced(
        &source,
        &[TraceCategory::Compile],
        TraceFilter::everything(),
        |interpreter| {
            interpreter.set_jit_zone_bytes(1 << 20);
            let stubs = interpreter.table_sizes().zone_used_bytes;
            interpreter.set_jit_zone_bytes(stubs + 4_544);
            interpreter.set_jit_stress(true);
            // The zone stress mode's compactions make room at every
            // safepoint, so that nothing need be held.
            #[cfg(debug_assertions)]
            interpreter.set_stress_zone(false);
        },
    );
    assert!(
        events.iter().any(|event| matches!(
            event.kind,
            TraceEventKind::CompileHeld {
                reason: HoldReason::ZoneFull,
                ..
            }
        )),
        "{events:?}"
    );
    // Between two holds of one body, a compile of it.
    let mut held_since_compiled: HashSet<&str> = HashSet::new();
    for event in &events {
        let Some(body) = event.body.as_deref() else {
            continue;
        };
        match event.kind {
            TraceEventKind::CompileHeld { .. } => {
                assert!(
                    held_since_compiled.insert(body),
                    "{body} held twice without a compile between: {events:?}"
                );
            }
            TraceEventKind::Compiled { .. } => {
                held_since_compiled.remove(body);
            }
            _ => {}
        }
    }
}

/// A breakpoint set on a body with an optimized copy frees the copy with
/// its baseline code: the freed copy's line names its key, as the
/// optimizing tier's lines do.
#[test]
fn a_freed_copy_is_traced_with_its_key() {
    let mut remote: Option<DebugRemote> = None;
    let events = traced_made_by(
        HOT_LOOP,
        Factory::Plain,
        &[TraceCategory::CodeZone, TraceCategory::Optimize],
        TraceFilter::everything(),
        |interpreter| {
            hot_loop_stress_off(interpreter);
            // The zone stress mode's compactions free the copy before
            // the breakpoint does.
            #[cfg(debug_assertions)]
            interpreter.set_stress_zone(false);
        },
        |interpreter| {
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
            true
        },
    );
    drop(remote);
    let run = about(&events, "Test>>run");
    assert!(
        run.iter().any(|event| matches!(
            &event.kind,
            TraceEventKind::Optimized { key: Some(key), .. } if key == "Test"
        )),
        "{events:?}"
    );
    assert!(
        run.iter().any(|event| matches!(
            &event.kind,
            TraceEventKind::Freed {
                key: Some(key),
                entered: None,
                tier: CodeTier::Optimized,
                cause: FreeCause::Breakpoint,
            } if key == "Test"
        )),
        "{events:?}"
    );
}

/// `maker:` makes four closures, each its frame's, once it is told to:
/// its twin, which reifies the frame at every one, is larger than its
/// lean body. Its first two entries, which compile it, make none, so
/// that nothing but `maker:` is compiled before its twin is needed.
const MAKING_CLOSURES: &str = indoc! {"
    class Test = Object new () (
      call: block = (
        ^block value
      )

      maker: making = (
        | sum ::= 0. |
        making ifFalse: [^0].
        sum:: sum + (self call: [1]).
        sum:: sum + (self call: [2]).
        sum:: sum + (self call: [3]).
        sum:: sum + (self call: [4]).
        ^sum.
      )

      public run = (
        ^(self maker: false) + (self maker: false) + (self maker: true) + (self maker: true)
      )
    )
"};

/// The bytes `body`, of `source`, and its twin take, measured in a zone
/// with room for both: the architecture's, and the build's.
fn body_and_twin_bytes(source: &str, body: &str) -> (usize, usize) {
    let events = traced(
        source,
        &[TraceCategory::Compile],
        TraceFilter::everything(),
        |_| {},
    );
    let compiled = about(&events, body);
    let lean = compiled.iter().find_map(|event| match event.kind {
        TraceEventKind::Compiled { bytes, .. } => Some(bytes),
        _ => None,
    });
    let twin = compiled.iter().find_map(|event| match event.kind {
        TraceEventKind::CompiledTwin { bytes } => Some(bytes),
        _ => None,
    });
    let (Some(lean), Some(twin)) = (lean, twin) else {
        panic!("`{body}` and its twin are compiled: {events:?}");
    };
    (lean, twin)
}

/// A zone with room for `maker:`'s lean body but not for its twin even
/// empty: the compiler refuses the twin for good, and with it the body,
/// whose machine code is freed -- every frame that makes a closure would
/// exit -- traced as such in both categories.
#[test]
fn a_body_whose_twin_no_zone_holds_is_traced_refused_for_its_twin() {
    let (lean, twin) = body_and_twin_bytes(MAKING_CLOSURES, "Test>>maker:");
    assert!(twin > lean, "the twin reifies the frame inline");
    let events = traced(
        MAKING_CLOSURES,
        &[TraceCategory::Compile, TraceCategory::CodeZone],
        TraceFilter::everything(),
        |interpreter| {
            interpreter.set_jit_zone_bytes(1 << 20);
            let stubs = interpreter.table_sizes().zone_used_bytes;
            interpreter.set_jit_zone_bytes(stubs + (lean + twin) / 2);
        },
    );
    let maker = about(&events, "Test>>maker:");
    assert!(
        maker
            .iter()
            .any(|event| event.category == TraceCategory::Compile
                && event.kind == TraceEventKind::RefusedForItsTwin),
        "{events:?}"
    );
    assert!(
        maker
            .iter()
            .any(|event| event.category == TraceCategory::CodeZone
                && matches!(
                    event.kind,
                    TraceEventKind::Freed {
                        tier: CodeTier::Baseline,
                        cause: FreeCause::RefusedForItsTwin,
                        ..
                    }
                )),
        "{events:?}"
    );
    assert!(
        !maker
            .iter()
            .any(|event| matches!(event.kind, TraceEventKind::CompiledTwin { .. })),
        "{events:?}"
    );
}

/// `maker:` compiled first, and running while a recursion through more
/// bodies than the zone holds fills it with active code, as in
/// `a_compile_the_exhausted_zone_holds_is_traced_once`; at the
/// recursion's end `maker:` runs again, makes a closure, which needs its
/// twin, and loops.
fn exhausting_maker(count: usize) -> String {
    let methods: String = (0..count)
        .map(|index| {
            let next = if index + 1 == count {
                "self maker: true".to_string()
            } else {
                format!("(self m{}) + 1", index + 1)
            };
            formatdoc! {"
                  m{index} = (
                    ^{next}
                  )
            "}
        })
        .collect();
    formatdoc! {"
        class Test = Object new () (
        {methods}  call: block = (
            ^block value
          )

          maker: deep = (
            |
              sum ::= 0.
              index ::= 0.
            |
            deep ifFalse: [^self m0].
            sum:: self call: [1].
            [index < 1000] whileTrue: [
              sum:: sum + index.
              index:: index + 1.
            ].
            ^sum
          )

          public run = (
            ^self maker: false
          )
        ) : ()
    "}
}

/// The exhausted zone holds `maker:`'s twin at its closure, traced once,
/// and its frame, which the interpreter reified for the closure and runs
/// on interpreted, cannot switch into the lean body at its loop's head:
/// a reified frame enters the twin only, which the zone still holds.
#[test]
fn a_twin_the_exhausted_zone_holds_is_traced_held_and_refusing_osr() {
    let source = exhausting_maker(300);
    let (lean, twin) = body_and_twin_bytes(&source, "Test>>maker:");
    let events = traced(
        &source,
        &[TraceCategory::Compile, TraceCategory::Osr],
        TraceFilter::everything(),
        |interpreter| {
            interpreter.set_jit_zone_bytes(1 << 20);
            let stubs = interpreter.table_sizes().zone_used_bytes;
            // Room for `maker:`'s lean body, then for the bodies of the
            // recursion that leave none for the twin, which an empty
            // zone would hold.
            interpreter.set_jit_zone_bytes(stubs + lean + twin);
            interpreter.set_jit_stress(true);
            // The zone stress mode's compactions make room at every
            // safepoint, so that nothing need be held, and the suspend
            // stress mode serves a safepoint at the loop's backward
            // jumps, rebuilding the stack, after which the jump tries no
            // switch.
            #[cfg(debug_assertions)]
            {
                interpreter.set_stress_zone(false);
                interpreter.set_stress_suspend(false);
            }
        },
    );
    let maker = about(&events, "Test>>maker:");
    assert_eq!(
        maker
            .iter()
            .filter(|event| event.category == TraceCategory::Compile
                && event.kind
                    == TraceEventKind::TwinHeld {
                        reason: HoldReason::ZoneFull
                    })
            .count(),
        1,
        "{events:?}"
    );
    assert!(
        maker
            .iter()
            .any(|event| event.category == TraceCategory::Osr
                && matches!(
                    event.kind,
                    TraceEventKind::OsrRefused {
                        reason: OsrRefusal::TwinHeld,
                        ..
                    }
                )),
        "{events:?}"
    );
}

/// `count`'s one backward jump spends its budget, a frame of `Cell` over
/// `Narrow`'s application at a time: the run-out at that jump leaves the
/// frame to the interpreter at the loop's head, whence it returns with
/// no safepoint between, so that the run ends with the request for
/// that key's copy pending.
#[cfg(debug_assertions)]
const ONE_BACK_JUMP_A_RUN: &str = indoc! {"
    class Subject = Object new () (
      class Narrow = Object new () ()

      class Box on: base = Object new (
        |
          base = base.
        |
      ) (
        public class Cell = base new () (
          public count = (
            | index ::= 0. |
            [index < 1] whileTrue: [
              index:: index + 1.
            ].
            ^index.
          )
        )
      )

      public narrow = (
        ^(Box on: Narrow) Cell new.
      )
    ) : ()
"};

/// A collection between runs that reclaims `Cell` over `Narrow`'s
/// application drops the request a run left pending for that key's
/// copy, which the next safepoint would have compiled for the dead id:
/// traced as dropped, naming the key's class as it was. Only a host's
/// collection between runs falls in that window -- a program's runs at
/// a safepoint, after the requests are compiled -- and it exists in a
/// build with debug assertions only.
#[cfg(debug_assertions)]
#[test]
fn a_request_a_reclaimed_id_drops_is_traced_dropped() {
    let module = ns_syntax::parse(ONE_BACK_JUMP_A_RUN).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        ONE_BACK_JUMP_A_RUN.into(),
        SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut interpreter = Interpreter::bootstrap();
    interpreter.set_jit_enabled(true);
    interpreter.set_jit_stress(false);
    // The optimize stress mode's leaf requests at a spend in machine
    // code, whose poll takes the safepoint at once.
    interpreter.set_optimize_stress(false);
    interpreter.set_optimization_budget(1);
    // Each serves a safepoint, or frees the code or the key, before the
    // collection this test makes.
    interpreter.set_stress_breakpoints(false);
    interpreter.set_stress_step(false);
    interpreter.set_stress_zone(false);
    interpreter.set_stress_collect(false);
    interpreter.set_stress_suspend(false);
    interpreter.set_trace(
        tracing(
            TraceCategories::default().with(TraceCategory::Optimize),
            TraceFilter::everything(),
        ),
        Box::new(Collector(Rc::clone(&events))),
    );
    let class = interpreter.load_module(&resolved);
    let subject = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let cell = expect_ok(
        interpreter.send(&subject, Symbol::intern("narrow"), &[]),
        "narrow",
    );
    let requested = |events: &[TraceEvent]| {
        events.iter().any(|event| {
            event.body.as_deref() == Some("Subject/Box.Cell>>count")
                && matches!(event.kind, TraceEventKind::OptimizationRequested { .. })
        })
    };
    let mut runs = 0;
    while !requested(&events.borrow()) {
        assert!(runs < 100, "`count` is requested: {:?}", events.borrow());
        expect_ok(
            interpreter.send(&cell, Symbol::intern("count"), &[]),
            "count",
        );
        runs += 1;
    }
    drop(cell);
    interpreter.collect_now();
    // A run delivers what the collection recorded.
    expect_ok(
        interpreter.send(&subject, Symbol::intern("narrow"), &[]),
        "narrow",
    );
    let events = events.take();
    assert!(
        events.iter().any(|event| matches!(
            &event.kind,
            TraceEventKind::OptimizationDropped {
                key: Some(key),
                cause: InvalidationCause::ReclaimedId,
                ..
            } if key == "Subject/Box.Cell"
        )),
        "{events:?}"
    );
}
