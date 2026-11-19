//! The developer tool's trace (`docs/design/developer-tool.md`, "Stage 1:
//! tracing"): the sink a host installs, and what it hears.

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

/// `HOT` loaded and run, after `prepare` was given the interpreter.
fn run_hot(prepare: impl FnOnce(&mut Interpreter)) {
    let module = ns_syntax::parse(HOT).expect("parse");
    let resolved = ns_runtime::resolve(module, HOT.into(), SourceOrigin::File("hot.ns".into()))
        .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    prepare(&mut interpreter);
    let class = interpreter.load_module(&resolved);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("run"), &[]),
        "run",
    );
}

/// Tracing costs nothing it is not asked for: without a sink, and with a
/// sink for no category, no event is recorded, so none is delivered.
#[test]
fn no_sink_no_events_and_no_cost() {
    let events = Rc::new(RefCell::new(Vec::new()));
    run_hot(|_| {});
    run_hot(|interpreter| {
        interpreter.set_trace(
            TraceCategories::default(),
            TraceFilter::everything(),
            Box::new(Collector(Rc::clone(&events))),
        );
    });
    assert!(events.borrow().is_empty());
}
