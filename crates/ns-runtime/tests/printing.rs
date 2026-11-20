//! The developer tool's printing of code (`docs/design/developer-tool.md`,
//! decision 10, "Stage 2: printing code"): the listings a sink the host
//! installs receives. A body's bytecode is listed at the first flush after
//! its mixin joined the heap's tables, whether or not it ever runs.

use std::cell::RefCell;
use std::rc::Rc;
use std::thread;
use std::time::Duration;

use indoc::indoc;
use ns_runtime::trace_events::{BytecodeListing, Listing, Printing, TraceSettings};
use ns_runtime::{
    Command, Event, HostResult, HostSignal, Interpreter, Root, SourceOrigin, StopReason, Symbol,
    TraceCategories, TraceCategory, TraceEvent, TraceFilter, TraceSink,
};

/// A sink that keeps every listing it receives where the test reads it.
struct Collector(Rc<RefCell<Vec<Listing>>>);

impl TraceSink for Collector {
    fn event(&mut self, _event: &TraceEvent) {}

    fn listing(&mut self, listing: &Listing) {
        self.0.borrow_mut().push(listing.clone());
    }
}

/// A module whose `sum:` makes a block, and whose `edit` installs a new
/// `sum:` through the MirrorBuilder and `check` only stages one.
const DEMO: &str = indoc! {"
    class Demo usingPlatform: platform = (
      |
        services = platform.
      |
    ) (
      public sum: list = (
        | total ::= 0. |
        list do: [:each | total:: total + each].
        ^total.
      )

      public run = (
        ^self sum: {1. 2. 3}.
      )

      builder = (
        ^(services mirrors ClassMirror reflecting: self class) mixin declaration asBuilder.
      )

      public edit = (
        | staged |
        staged:: self builder.
        staged instanceSide methods addFromSource: 'public sum: list = ( ^list size )'.
        staged install.
      )

      public check = (
        | staged |
        staged:: self builder.
        staged instanceSide methods addFromSource: 'public sum: list = ( ^list size )'.
      )

      public time = (
        ^services time.
      )
    )
"};

/// How long the remote's thread waits for an event before it fails the
/// test: long enough for a debug build under a loaded machine.
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// `DEMO` loaded and instantiated with the interpreter's platform.
fn instance() -> (Interpreter, Root) {
    let module = ns_syntax::parse(DEMO).expect("parse");
    let resolved = ns_runtime::resolve(module, DEMO.into(), SourceOrigin::File("demo.ns".into()))
        .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    let platform = interpreter.platform();
    let instance = expect_ok(
        interpreter.send(&class, Symbol::intern("usingPlatform:"), &[platform]),
        "usingPlatform:",
    );
    (interpreter, instance)
}

/// Installs a sink printing bytecode for the bodies `pattern` selects,
/// tracing `categories`; answers what it collects.
fn print_bytecode(
    interpreter: &mut Interpreter,
    categories: TraceCategories,
    pattern: &str,
) -> Rc<RefCell<Vec<Listing>>> {
    let listings = Rc::new(RefCell::new(Vec::new()));
    interpreter.set_trace(
        TraceSettings {
            categories,
            filter: TraceFilter::everything(),
            printing: Printing {
                bytecode: true,
                ..Printing::default()
            },
            print_filter: TraceFilter::parse(&[pattern]).expect("a valid filter"),
        },
        Box::new(Collector(Rc::clone(&listings))),
    );
    listings
}

/// The bytecode listings among `listings`.
fn bytecode(listings: &[Listing]) -> Vec<&BytecodeListing> {
    listings
        .iter()
        .map(|listing| match listing {
            Listing::Bytecode(bytecode) => bytecode,
        })
        .collect()
}

/// The bytecode listings of the body named `body`.
fn listings_of(listings: &[Listing], body: &str) -> Vec<BytecodeListing> {
    bytecode(listings)
        .into_iter()
        .filter(|listing| listing.body == body)
        .cloned()
        .collect()
}

/// Asserts that `listings` list `Demo>>sum:` with its block nested, and no
/// body of the core library.
fn assert_lists_sum(listings: &[Listing]) {
    let sums = listings_of(listings, "Demo>>sum:");
    assert_eq!(sums.len(), 1, "one listing of the body");
    assert!(sums[0].text.contains("MakeClosure"), "{}", sums[0].text);
    assert!(sums[0].text.contains("block"), "{}", sums[0].text);
    assert!(
        bytecode(listings)
            .iter()
            .all(|listing| !listing.body.contains("newspeak.core")),
        "the filter selects the program's bodies only"
    );
}

#[test]
fn the_programs_bodies_print_when_the_sink_is_installed() {
    let (mut interpreter, _) = instance();
    let listings = print_bytecode(
        &mut interpreter,
        TraceCategories::default().with(TraceCategory::Compile),
        "Demo...",
    );
    assert_lists_sum(&listings.borrow());
}

#[test]
fn printing_needs_no_trace_category() {
    let (mut interpreter, _) = instance();
    let listings = print_bytecode(&mut interpreter, TraceCategories::default(), "Demo...");
    assert_lists_sum(&listings.borrow());
}

#[test]
fn a_shipped_module_built_on_demand_prints_at_the_next_flush() {
    let (mut interpreter, instance) = instance();
    let listings = print_bytecode(
        &mut interpreter,
        TraceCategories::default(),
        "newspeak.core.TimeFactory...",
    );
    assert!(
        listings.borrow().is_empty(),
        "the time module is not built before the run"
    );
    expect_ok(
        interpreter.send(&instance, Symbol::intern("time"), &[]),
        "time",
    );
    assert!(
        bytecode(&listings.borrow())
            .iter()
            .any(|listing| listing.body.starts_with("newspeak.core.TimeFactory/")),
        "the time module's bodies listed after the run"
    );
}

#[test]
fn an_installed_edit_prints_its_new_bodies() {
    let (mut interpreter, instance) = instance();
    let listings = print_bytecode(&mut interpreter, TraceCategories::default(), "Demo...");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("edit"), &[]),
        "edit",
    );
    let sums = listings_of(&listings.borrow(), "Demo>>sum:");
    assert_eq!(sums.len(), 2, "the body loaded and the body installed");
    assert!(sums[0].text.contains("MakeClosure"), "{}", sums[0].text);
    assert!(!sums[1].text.contains("MakeClosure"), "{}", sums[1].text);
    assert!(sums[1].text.contains("#size"), "{}", sums[1].text);
}

#[test]
fn a_checked_edit_prints_nothing() {
    let (mut interpreter, instance) = instance();
    let listings = print_bytecode(&mut interpreter, TraceCategories::default(), "Demo...");
    let before = listings.borrow().len();
    assert!(before > 0, "the program's bodies listed at the install");
    expect_ok(
        interpreter.send(&instance, Symbol::intern("check"), &[]),
        "check",
    );
    assert_eq!(
        listings.borrow().len(),
        before,
        "a checked edit makes no body"
    );
}

#[test]
fn the_debuggers_metadata_compile_prints_nothing() {
    let (mut interpreter, instance) = instance();
    let listings = print_bytecode(&mut interpreter, TraceCategories::default(), "Demo...");
    let before = listings.borrow().len();
    assert!(before > 0, "the program's bodies listed at the install");
    let remote = interpreter.attach_debugger();
    remote.pause();
    let agent = thread::spawn(move || {
        match remote.next_event(EVENT_TIMEOUT) {
            Some(Event::Stopped {
                reason: StopReason::Pause,
                ..
            }) => {}
            other => panic!("a stop for the pause was expected, not {other:?}"),
        }
        remote.send(Command::Variables { frame: 0 });
        match remote.next_event(EVENT_TIMEOUT) {
            Some(Event::Variables(variables)) => assert!(!variables.is_empty()),
            other => panic!("variables were expected, not {other:?}"),
        }
        remote.send(Command::Continue);
        match remote.next_event(EVENT_TIMEOUT) {
            Some(Event::Continued) => {}
            other => panic!("the confirmation of the continue was expected, not {other:?}"),
        }
        remote
    });
    expect_ok(
        interpreter.send(&instance, Symbol::intern("run"), &[]),
        "run",
    );
    let remote = agent.join().expect("the remote's thread");
    drop(remote);
    assert_eq!(
        listings.borrow().len(),
        before,
        "compiling the metadata again makes no body"
    );
}
