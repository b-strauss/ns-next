//! The developer tool's trace (`docs/design/developer-tool.md`, "Stage 1:
//! tracing"): the sink a host installs, and what it hears. These pin
//! what the JIT's sites record, so they run with the JIT on whatever the
//! environment says, as `tests/jit.rs` does.
#![cfg(ns_jit)]

use std::cell::RefCell;
use std::rc::Rc;

use indoc::indoc;
use ns_runtime::{
    HostResult, HostSignal, Interpreter, Root, SourceOrigin, Symbol, TraceCategories, TraceEvent,
    TraceFilter, TraceSink,
};

/// A sink that keeps every event it receives where the test reads it.
struct Collector(Rc<RefCell<Vec<TraceEvent>>>);

impl TraceSink for Collector {
    #[expect(
        unreachable_code,
        reason = "no event kind exists yet, so no event is ever received"
    )]
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
    let module = ns_syntax::parse(HOT).expect("parse");
    let resolved = ns_runtime::resolve(module, HOT.into(), SourceOrigin::File("hot.ns".into()))
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
