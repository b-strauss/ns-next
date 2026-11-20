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
        .filter_map(|listing| match listing {
            Listing::Bytecode(bytecode) => Some(bytecode),
            Listing::Code(_) => None,
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

/// The machine code of each body installed, listed as it was installed
/// (`docs/design/developer-tool.md`, decisions 4, 5 and 14). These pin
/// what the JIT installs, so they run with the JIT on whatever the
/// environment says, as `tests/trace.rs` does.
#[cfg(ns_jit)]
mod code {
    use std::cell::RefCell;
    use std::rc::Rc;

    use indoc::indoc;
    use ns_runtime::trace_events::{
        Annotation, Architecture, CodeListing, CodeTier, DataWord, Listing, Position, Printing,
        TraceSettings,
    };
    use ns_runtime::{Interpreter, Root, SourceOrigin, Symbol, TraceCategories, TraceFilter};

    use super::{Collector, expect_ok};

    /// A module whose `square:` sends `times:by:`, whose `total:` makes a
    /// block and whose `cube:` is another body for the filter to leave
    /// out, each entered by the unary method that sends it.
    const HOT: &str = indoc! {"
        class Demo = Object new () (
          public square: number = (
            ^self times: number by: number.
          )

          public cube: number = (
            ^(self times: number by: number) * number.
          )

          public times: left by: right = (
            ^left * right.
          )

          public total: list = (
            | sum ::= 0. |
            list do: [:each | sum:: sum + each].
            ^sum.
          )

          public runSquare = (
            ^self square: 7.
          )

          public runCube = (
            ^self cube: 3.
          )

          public runTotal = (
            ^self total: {1. 2. 3}.
          )
        )
    "};

    /// `HOT` loaded and instantiated with the JIT on, the machine code of
    /// the bodies `pattern` selects printed into the listings answered.
    fn hot(pattern: &str) -> (Interpreter, Root, Rc<RefCell<Vec<Listing>>>) {
        let module = ns_syntax::parse(HOT).expect("parse");
        let resolved =
            ns_runtime::resolve(module, HOT.into(), SourceOrigin::File("demo.ns".into()))
                .expect("resolve");
        let mut interpreter = Interpreter::bootstrap();
        // On whatever the environment says, every body compiled at its
        // second entry: the listings are the JIT's installs.
        interpreter.set_jit_enabled(true);
        interpreter.set_jit_stress(false);
        // Off whatever the environment says: a body the breakpoint or
        // step stress mode patched loses its machine code at every
        // statement, and one the zone stress mode freed is compiled and
        // listed again.
        #[cfg(debug_assertions)]
        {
            interpreter.set_stress_breakpoints(false);
            interpreter.set_stress_step(false);
            interpreter.set_stress_zone(false);
        }
        let class = interpreter.load_module(&resolved);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let listings = Rc::new(RefCell::new(Vec::new()));
        interpreter.set_trace(
            TraceSettings {
                categories: TraceCategories::default(),
                filter: TraceFilter::everything(),
                printing: Printing {
                    code: true,
                    ..Printing::default()
                },
                print_filter: TraceFilter::parse(&[pattern]).expect("a valid filter"),
            },
            Box::new(Collector(Rc::clone(&listings))),
        );
        (interpreter, instance, listings)
    }

    /// Sends the unary `selector` to `instance` `times` times.
    fn enter(interpreter: &mut Interpreter, instance: &Root, selector: &str, times: usize) {
        for _ in 0..times {
            expect_ok(
                interpreter.send(instance, Symbol::intern(selector), &[]),
                selector,
            );
        }
    }

    /// The code listings of `body` compiled as `tier`.
    fn code_of(listings: &[Listing], body: &str, tier: CodeTier) -> Vec<CodeListing> {
        listings
            .iter()
            .filter_map(|listing| match listing {
                Listing::Code(code) if code.body == body && code.tier == tier => Some(code.clone()),
                _ => None,
            })
            .collect()
    }

    /// The host's architecture, as a listing names it.
    fn host() -> Architecture {
        if cfg!(target_arch = "aarch64") {
            Architecture::Aarch64
        } else {
            Architecture::X64
        }
    }

    #[test]
    fn a_hot_method_prints_its_baseline_code() {
        let (mut interpreter, instance, listings) = hot("Demo>>square:");
        enter(&mut interpreter, &instance, "runSquare", 2);
        let listings = listings.borrow();
        let squares = code_of(&listings, "Demo>>square:", CodeTier::Baseline);
        let [square] = squares.as_slice() else {
            panic!("one baseline listing of the body, not {}", squares.len());
        };
        assert_eq!(square.architecture, host());
        assert_ne!(square.base_address, 0);
        assert_eq!(square.key, None);
        assert_eq!(square.mark, None);
        assert!(square.code_end as usize <= square.bytes.len());
        let [scope] = square.scopes.as_slice() else {
            panic!("a baseline listing has one scope");
        };
        assert_eq!((scope.number, scope.parent), (0, None));
        assert_eq!(scope.body, "Demo>>square:");
        assert!(
            scope
                .instructions
                .iter()
                .any(|line| line.contains("#times:by:")),
            "{:?}",
            scope.instructions
        );
        // A position at each instruction's start, in program-counter
        // order, inside the code.
        let program_counters: Vec<u32> = square
            .positions
            .iter()
            .filter_map(|(offset, position)| match position {
                Position::Instruction {
                    scope: 0,
                    program_counter,
                } => {
                    assert!(*offset < square.code_end);
                    Some(*program_counter)
                }
                Position::Instruction { .. } => panic!("a baseline listing has one scope"),
                Position::OutOfLine => None,
            })
            .collect();
        let expected: Vec<u32> = (0..).take(scope.instructions.len()).collect();
        assert_eq!(program_counters, expected);
        assert!(square.positions.is_sorted_by_key(|(offset, _)| *offset));
        assert!(
            square
                .annotations
                .iter()
                .any(|(_, annotation)| matches!(annotation, Annotation::Cell { site: 0 })),
            "the send's call through its cell: {:?}",
            square.annotations
        );
        assert!(
            square
                .annotations
                .iter()
                .all(|(offset, _)| *offset < square.code_end)
        );
    }

    #[test]
    fn a_twin_prints_as_a_twin() {
        let (mut interpreter, instance, listings) = hot("Demo>>total:");
        enter(&mut interpreter, &instance, "runTotal", 3);
        let listings = listings.borrow();
        let tier_at = |tier: CodeTier| {
            listings
                .iter()
                .position(|listing| matches!(listing, Listing::Code(code) if code.tier == tier))
        };
        let (Some(lean), Some(twin)) = (tier_at(CodeTier::Baseline), tier_at(CodeTier::Twin))
        else {
            panic!("the lean body and its twin listed");
        };
        assert!(lean < twin, "the twin is compiled once the closure is made");
        assert_eq!(
            code_of(&listings, "Demo>>total:", CodeTier::Twin).len(),
            1,
            "the twin is compiled once"
        );
        let leans = code_of(&listings, "Demo>>total:", CodeTier::Baseline);
        assert!(
            leans[0]
                .words
                .iter()
                .any(|(_, word)| matches!(word, DataWord::TwinTransfer { .. })),
            "the lean body's transfer into its twin: {:?}",
            leans[0].words
        );
        assert!(
            leans[0]
                .annotations
                .iter()
                .any(|(_, annotation)| *annotation == Annotation::Target("twin")),
            "the transfer's jump into the twin named: {:?}",
            leans[0].annotations
        );
    }

    #[test]
    fn a_data_word_is_named_not_shown() {
        let (mut interpreter, instance, listings) = hot("Demo>>square:");
        enter(&mut interpreter, &instance, "runSquare", 2);
        let listings = listings.borrow();
        let square = &code_of(&listings, "Demo>>square:", CodeTier::Baseline)[0];
        let kinds: Vec<DataWord> = square.words.iter().map(|(_, word)| *word).collect();
        for kind in [
            DataWord::Budget,
            DataWord::DispatchingRows,
            DataWord::DispatchingTail,
        ] {
            assert!(kinds.contains(&kind), "{kind:?} among {kinds:?}");
        }
        let first = square
            .words
            .first()
            .expect("a method's lean body has words")
            .0;
        assert_eq!(
            first, square.code_end,
            "the words begin where the code ends"
        );
        for (offset, _) in &square.words {
            let start = *offset as usize;
            assert_eq!(
                square.bytes.get(start..start + 8),
                Some([0u8; 8].as_slice()),
                "a word's value is the run's, never shown"
            );
        }
    }

    #[test]
    fn the_filter_selects_code_listings_by_body() {
        let (mut interpreter, instance, listings) = hot("Demo>>square:");
        enter(&mut interpreter, &instance, "runSquare", 2);
        enter(&mut interpreter, &instance, "runCube", 2);
        let listings = listings.borrow();
        assert!(!listings.is_empty());
        for listing in listings.iter() {
            let Listing::Code(code) = listing else {
                panic!("only machine code is printed");
            };
            assert_eq!(code.body, "Demo>>square:");
        }
        // The same run with the filter on `cube:` lists it: the body the
        // first filter left out was compiled.
        let (mut interpreter, instance, cubes) = hot("Demo>>cube:");
        enter(&mut interpreter, &instance, "runSquare", 2);
        enter(&mut interpreter, &instance, "runCube", 2);
        assert_eq!(
            code_of(&cubes.borrow(), "Demo>>cube:", CodeTier::Baseline).len(),
            1,
            "the body the filter left out was compiled"
        );
    }
}
