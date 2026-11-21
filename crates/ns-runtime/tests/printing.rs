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
            describe_bodies: false,
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
            Listing::Code(_) | Listing::Inlining(_) => None,
        })
        .collect()
}

/// The bytecode listings of the body named `body`.
fn listings_of(listings: &[Listing], body: &str) -> Vec<BytecodeListing> {
    bytecode(listings)
        .into_iter()
        .filter(|listing| listing.body.name == body)
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
            .all(|listing| !listing.body.name.contains("newspeak.core")),
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
fn a_bytecode_listing_lists_each_body_structured() {
    let (mut interpreter, _) = instance();
    let listings = print_bytecode(&mut interpreter, TraceCategories::default(), "Demo...");
    let sums = listings_of(&listings.borrow(), "Demo>>sum:");
    assert_eq!(sums.len(), 1);
    let listing = &sums[0];
    assert_eq!(listing.bodies.len(), 2, "the method and its one block");
    assert_eq!(listing.bodies[0].body, listing.body);
    assert_eq!(listing.bodies[1].body.name, "[] in Demo>>sum:");
    assert_ne!(listing.bodies[1].body.number, 0);
    assert_ne!(listing.bodies[1].body.number, listing.body.number);
    // Each body's lines, formatted as `instruction_lines` does, appear in
    // the text in order (a block's lines nested under its `MakeClosure`).
    let text_lines: Vec<&str> = listing.text.lines().map(str::trim_start).collect();
    for body in &listing.bodies {
        let mut remaining = text_lines.iter();
        for (expected, instruction) in body.instructions.iter().enumerate() {
            assert_eq!(usize::try_from(instruction.index).unwrap(), expected);
            let statement = instruction
                .statement
                .map_or("-".to_string(), |statement| format!("stmt {statement}"));
            let line = format!(
                "{:4}  {statement:>7}  {}",
                instruction.index, instruction.text
            );
            assert!(
                remaining.any(|actual| *actual == line.trim_start()),
                "{line} is not in order in
{}",
                listing.text
            );
        }
    }
    // Each body's line above its instructions is the text's: the root's
    // first, named as the trace names it, a block's as the text names it.
    let lines: Vec<&str> = listing.text.lines().map(str::trim_start).collect();
    for (position, body) in listing.bodies.iter().enumerate() {
        let name = if position == 0 {
            listing.body.name.as_str()
        } else {
            "block"
        };
        let line = format!(
            "{} {name} (args {}, registers {})",
            body.kind, body.argument_count, body.register_count
        );
        assert!(
            lines.contains(&line.as_str()),
            "{line} is not in\n{}",
            listing.text
        );
    }
    assert_eq!(listing.bodies[0].kind, "method");
    assert_eq!(listing.bodies[1].kind, "block");
    assert_eq!(listing.bodies[1].argument_count, 1);
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
            .any(|listing| listing.body.name.starts_with("newspeak.core.TimeFactory/")),
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

    use indoc::{formatdoc, indoc};
    use ns_runtime::trace_events::{
        Annotation, Architecture, ArmKind, CodeListing, CodeTier, CompileRefusal, DataWord,
        InlineDecline, InliningListing, InliningOutcome, ListedArm, Listing, Position, Printing,
        TraceSettings,
    };
    use ns_runtime::{
        Interpreter, Root, SourceOrigin, Symbol, TraceCategories, TraceCategory, TraceEvent,
        TraceEventKind, TraceFilter, TraceSink,
    };

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
                describe_bodies: false,
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
                Listing::Code(code) if code.body.name == body && code.tier == tier => {
                    Some(code.clone())
                }
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
        assert_eq!(scope.body.name, "Demo>>square:");
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
        // The words begin at the next cache line past the code; the
        // padding between is no instruction, and a listing decodes none.
        assert!(
            square.code_end <= first && first - square.code_end < 64,
            "the words at {first:#x} follow the code's end at {:#x}",
            square.code_end
        );
        assert!(
            square.bytes[square.code_end as usize..first as usize]
                .iter()
                .all(|byte| *byte == 0),
            "the padding is zeroed"
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
            assert_eq!(code.body.name, "Demo>>square:");
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

    /// A module whose `total`'s loop sends `balance` to an `Account` a
    /// million times: its optimized copy inlines `balance`, a method with
    /// a body of its own, as a scope.
    const INLINING: &str = indoc! {"
        class Demo = Object new () (
          class Account = (
            |
              amount ::= 10.
            |
          ) (
            public balance = (
              | doubled = amount * 2. |
              ^doubled - amount.
            )
          )

          public total = (
            |
              account = Account new.
              sum ::= 0.
              index ::= 0.
            |
            [index < 1000000] whileTrue: [
              sum:: sum + account balance.
              index:: index + 1.
            ].
            ^sum.
          )
        )
    "};

    /// What a sink receives, in its order.
    enum Received {
        Event(TraceEvent),
        Listing(Listing),
    }

    /// A sink that keeps every event and listing it receives, in order.
    struct Recorder(Rc<RefCell<Vec<Received>>>);

    impl TraceSink for Recorder {
        fn event(&mut self, event: &TraceEvent) {
            self.0.borrow_mut().push(Received::Event(event.clone()));
        }

        fn listing(&mut self, listing: &Listing) {
            self.0.borrow_mut().push(Received::Listing(listing.clone()));
        }
    }

    /// `INLINING`'s `total` run once with the JIT on, its budget the JIT's
    /// own, tracing the optimize and inlining categories and printing the
    /// machine code of `Demo>>total`: what the sink received.
    fn optimized_total() -> Vec<Received> {
        let printing = Printing {
            code: true,
            ..Printing::default()
        };
        run_total(INLINING, printing, |_| {}).1
    }

    /// `source`'s `total` run once with the JIT on, its budget the JIT's
    /// own, `prepare` run on the interpreter before the module is loaded,
    /// tracing the optimize and inlining categories and printing what
    /// `printing` names of `Demo>>total`: the interpreter and what the sink
    /// received.
    fn run_total(
        source: &str,
        printing: Printing,
        prepare: impl FnOnce(&mut Interpreter),
    ) -> (Interpreter, Vec<Received>) {
        let module = ns_syntax::parse(source).expect("parse");
        let resolved =
            ns_runtime::resolve(module, source.into(), SourceOrigin::File("demo.ns".into()))
                .expect("resolve");
        let mut interpreter = Interpreter::bootstrap();
        // On whatever the environment says, every body compiled at its
        // second entry and optimized when its budget runs out: the
        // listings are the JIT's installs, the plan made of what ran.
        interpreter.set_jit_enabled(true);
        interpreter.set_jit_stress(false);
        interpreter.set_optimize_stress(false);
        // Off whatever the environment says: a body the breakpoint or
        // step stress mode patched loses its machine code at every
        // statement, the suspend stress mode serves a safepoint at the
        // loop's backward jumps, so that the loop is never compiled, and
        // the cache stress mode vacates the caches at every poll, so that
        // the plan finds no receiver the caches saw, and the zone stress
        // mode frees the loop's code at every safepoint, so that its
        // budget never runs out.
        #[cfg(debug_assertions)]
        {
            interpreter.set_stress_breakpoints(false);
            interpreter.set_stress_step(false);
            interpreter.set_stress_suspend(false);
            interpreter.set_stress_caches(false);
            interpreter.set_stress_zone(false);
        }
        prepare(&mut interpreter);
        let class = interpreter.load_module(&resolved);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        let received = Rc::new(RefCell::new(Vec::new()));
        interpreter.set_trace(
            TraceSettings {
                categories: TraceCategories::default()
                    .with(TraceCategory::Optimize)
                    .with(TraceCategory::Inlining),
                filter: TraceFilter::everything(),
                printing,
                print_filter: TraceFilter::parse(&["Demo>>total"]).expect("a valid filter"),
                describe_bodies: false,
            },
            Box::new(Recorder(Rc::clone(&received))),
        );
        expect_ok(
            interpreter.send(&instance, Symbol::intern("total"), &[]),
            "total",
        );
        (interpreter, received.take())
    }

    /// The optimized listings of `Demo>>total` among `received`.
    fn optimized_listings(received: &[Received]) -> Vec<CodeListing> {
        received
            .iter()
            .filter_map(|item| match item {
                Received::Listing(Listing::Code(code))
                    if code.body.name == "Demo>>total" && code.tier == CodeTier::Optimized =>
                {
                    Some(code.clone())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_optimized_copy_lists_its_scopes() {
        let received = optimized_total();
        let listings = optimized_listings(&received);
        let Some(listing) = listings.first() else {
            panic!("the copy of `total` listed");
        };
        assert!(listing.code_end as usize <= listing.bytes.len());
        let root = &listing.scopes[0];
        assert_eq!(
            (root.number, root.body.name.as_str(), root.parent),
            (0, "Demo>>total", None)
        );
        let Some(balance) = listing
            .scopes
            .iter()
            .find(|scope| scope.body.name == "Demo/Account>>balance")
        else {
            panic!("`balance` inlined as a scope: {:?}", listing.scopes);
        };
        let Some((0, program_counter)) = balance.parent else {
            panic!("`balance` inlined into the body's own scope");
        };
        assert!(
            root.instructions[program_counter as usize].contains("#balance"),
            "inlined at the send of `balance`: {}",
            root.instructions[program_counter as usize]
        );
        assert!(
            !balance.instructions.is_empty(),
            "the scope's instructions listed"
        );
        for (index, scope) in listing.scopes.iter().enumerate() {
            assert_eq!(scope.number as usize, index, "the scopes numbered densely");
        }
        assert!(
            listing.positions.iter().any(|(_, position)| matches!(
                position,
                Position::Instruction { scope, .. } if *scope == balance.number
            )),
            "a stretch of code in `balance`'s scope: {:?}",
            listing.positions
        );
        // Every fast path of the body's own -- `index < …`, `sum + …` and
        // `index + 1`, the last two past `balance`'s scope -- names its
        // own instruction where the straight line runs it, before the
        // code of a later instruction, not only at its trap out of line,
        // which follows all of them.
        let program_counters: Vec<u32> = listing
            .positions
            .iter()
            .filter_map(|(_, position)| match position {
                Position::Instruction {
                    scope: 0,
                    program_counter,
                } => Some(*program_counter),
                _ => None,
            })
            .collect();
        let fast_paths: Vec<u32> = (0u32..)
            .zip(&root.instructions)
            .filter(|(_, line)| line.contains("IntegerBinary"))
            .map(|(program_counter, _)| program_counter)
            .collect();
        assert!(fast_paths.len() >= 3, "{:?}", root.instructions);
        for fast_path in fast_paths {
            let in_line = program_counters
                .iter()
                .position(|program_counter| *program_counter == fast_path)
                .is_some_and(|index| {
                    program_counters[index + 1..]
                        .iter()
                        .any(|later| *later > fast_path)
                });
            assert!(
                in_line,
                "s0@{fast_path} in the straight line: {:?}",
                listing.positions
            );
        }
        assert_eq!(
            listing.positions.first().map(|(offset, _)| *offset),
            Some(0)
        );
        assert!(
            listing
                .positions
                .windows(2)
                .all(|pair| pair[0].0 < pair[1].0 && pair[0].1 != pair[1].1),
            "a position where the code's position changes: {:?}",
            listing.positions
        );
        assert!(
            listing.positions.iter().all(|(offset, position)| {
                *offset < listing.code_end
                    && match position {
                        Position::Instruction {
                            scope,
                            program_counter,
                        } => listing.scopes.get(*scope as usize).is_some_and(|listed| {
                            (*program_counter as usize) < listed.instructions.len()
                        }),
                        Position::OutOfLine => true,
                    }
            }),
            "{:?}",
            listing.positions
        );
    }

    #[test]
    fn the_scopes_of_a_listing_are_the_plans() {
        let received = optimized_total();
        // The inlining events since the request, the plan of the compile
        // the next `optimized` event installs.
        let mut window = Vec::new();
        let mut planned = None;
        let mut compared = 0;
        for item in &received {
            match item {
                Received::Event(event)
                    if event.body.as_ref().map(|body| body.name.as_str())
                        == Some("Demo>>total") =>
                {
                    match &event.kind {
                        TraceEventKind::Inlined {
                            in_body,
                            program_counter,
                            callee,
                            ..
                        } => window.push((
                            in_body.name.clone(),
                            *program_counter,
                            callee.name.clone(),
                        )),
                        TraceEventKind::Optimized { .. } => {
                            planned = Some(std::mem::take(&mut window));
                        }
                        _ if event.category == TraceCategory::Optimize => window.clear(),
                        _ => {}
                    }
                }
                Received::Listing(Listing::Code(code))
                    if code.body.name == "Demo>>total" && code.tier == CodeTier::Optimized =>
                {
                    let Some(mut planned) = planned.take() else {
                        panic!("the listing follows its compile's event");
                    };
                    let mut listed: Vec<(String, u32, String)> = code
                        .scopes
                        .iter()
                        .filter_map(|scope| {
                            let (parent, program_counter) = scope.parent?;
                            Some((
                                code.scopes[parent as usize].body.name.clone(),
                                program_counter,
                                scope.body.name.clone(),
                            ))
                        })
                        .collect();
                    planned.sort();
                    listed.sort();
                    assert!(!listed.is_empty(), "the copy inlines");
                    assert_eq!(listed, planned);
                    compared += 1;
                }
                _ => {}
            }
        }
        assert!(compared > 0, "an optimized listing of `total`");
    }

    /// Only the plans are printed.
    fn plans_only() -> Printing {
        Printing {
            inlining: true,
            ..Printing::default()
        }
    }

    /// The inlining listings of `Demo>>total` among `received`.
    fn plans_of_total(received: &[Received]) -> Vec<InliningListing> {
        received
            .iter()
            .filter_map(|item| match item {
                Received::Listing(Listing::Inlining(plan)) if plan.body.name == "Demo>>total" => {
                    Some(plan.clone())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_installed_optimization_prints_its_plan() {
        let (_, received) = run_total(INLINING, plans_only(), |_| {});
        let plans = plans_of_total(&received);
        let Some(plan) = plans.first() else {
            panic!("the plan of `total` listed");
        };
        assert_eq!(plan.key, Some(("Demo".to_string(), None)));
        assert_eq!(
            (plan.scopes[0].number, plan.scopes[0].body.name.as_str()),
            (0, "Demo>>total")
        );
        let Some(balance) = plan
            .scopes
            .iter()
            .find(|scope| scope.body.name == "Demo/Account>>balance")
        else {
            panic!("`balance` inlined as a scope: {:?}", plan.scopes);
        };
        assert_eq!(balance.number, 1);
        let Some((0, program_counter)) = balance.parent else {
            panic!("`balance` inlined into the body's own scope");
        };
        assert!(
            plan.scopes
                .iter()
                .all(|scope| scope.instructions.is_empty()),
            "a plan lists no instructions"
        );
        assert!(
            plan.arms.contains(&ListedArm {
                scope: 0,
                program_counter,
                receiver: "Demo/Account".to_string(),
                inlined: ArmKind::Scope(1),
            }),
            "the type-case's arm for `Account` inlines `balance`: {:?}",
            plan.arms
        );
        assert!(
            plan.arms
                .is_sorted_by_key(|arm| (arm.scope, arm.program_counter)),
            "{:?}",
            plan.arms
        );
        // The listing follows the compile's event, and says what it said.
        let optimized = received.iter().position(|item| {
            matches!(item, Received::Event(event)
                if event.body.as_ref().map(|body| body.name.as_str()) == Some("Demo>>total")
                    && matches!(event.kind, TraceEventKind::Optimized { .. }))
        });
        let listed = received
            .iter()
            .position(|item| matches!(item, Received::Listing(Listing::Inlining(_))));
        let (Some(optimized), Some(listed)) = (optimized, listed) else {
            panic!("the compile's event and its plan");
        };
        assert!(optimized < listed, "the plan follows its compile's event");
        let Received::Event(TraceEvent {
            kind: TraceEventKind::Optimized { bytes, .. },
            ..
        }) = &received[optimized]
        else {
            unreachable!("found above");
        };
        assert_eq!(plan.outcome, InliningOutcome::Installed { bytes: *bytes });
    }

    #[test]
    fn a_refused_optimization_prints_its_plan() {
        let printing = Printing {
            code: true,
            inlining: true,
            ..Printing::default()
        };
        let roomy = |interpreter: &mut Interpreter| {
            interpreter.set_jit_zone_bytes(1 << 20);
            // The zone stress mode's compactions and frees change what
            // the zone holds when the copy is compiled.
            #[cfg(debug_assertions)]
            interpreter.set_stress_zone(false);
        };
        // The zone the run fills with room for every copy, and the copies'
        // bytes: the architecture's, and the build's.
        let (interpreter, received) = run_total(INLINING, printing, roomy);
        let used = interpreter.table_sizes().zone_used_bytes;
        let copies: Vec<usize> = received
            .iter()
            .filter_map(|item| match item {
                Received::Event(TraceEvent {
                    kind: TraceEventKind::Optimized { bytes, .. },
                    ..
                }) => Some(*bytes),
                _ => None,
            })
            .collect();
        let Some(smallest) = copies.iter().min() else {
            panic!("a copy installed with room");
        };
        // Room for the baseline code, and for half the smallest copy.
        let zone = used - copies.iter().sum::<usize>() + smallest / 2;
        let (_, received) = run_total(INLINING, printing, |interpreter| {
            roomy(interpreter);
            interpreter.set_jit_zone_bytes(zone);
        });
        let plans = plans_of_total(&received);
        assert!(
            plans.iter().any(|plan| plan.outcome
                == InliningOutcome::Refused(CompileRefusal::ZoneFull)
                && plan
                    .scopes
                    .iter()
                    .any(|scope| scope.body.name == "Demo/Account>>balance")),
            "the plan of the copy no room held: {:?}",
            plans.iter().map(|plan| &plan.outcome).collect::<Vec<_>>()
        );
        assert!(
            plans
                .iter()
                .all(|plan| !matches!(plan.outcome, InliningOutcome::Installed { .. })),
            "no copy installed"
        );
        assert!(
            optimized_listings(&received).is_empty(),
            "no machine code of a copy refused"
        );
    }

    /// A module whose `total`'s loop sends `value:` to a block it made,
    /// which weighs more than a block the plan inlines may: thirty sends
    /// of `step:`, a method with no fast path.
    fn heavy() -> String {
        let steps: String = (0..30)
            .map(|_| "        value:: self step: value.\n")
            .collect();
        formatdoc! {"
            class Demo = Object new () (
              public step: number = (
                ^number + 1
              )

              public total = (
                |
                  sum ::= 0.
                  index ::= 0.
                  heavy = [:number |
                    | value ::= number. |
            {steps}        value
                  ].
                |
                [index < 1000000] whileTrue: [
                  sum:: sum + (heavy value: index).
                  index:: index + 1.
                ].
                ^sum.
              )
            )
        "}
    }

    #[test]
    fn a_decline_names_its_scope() {
        let (_, received) = run_total(&heavy(), plans_only(), |_| {});
        let plans = plans_of_total(&received);
        assert!(
            plans
                .iter()
                .any(|plan| plan.declined.iter().any(|declined| {
                    declined.scope == 0
                        && declined.reason == InlineDecline::OverWeight
                        && declined.callee.as_ref().map(|callee| callee.name.as_str())
                            == Some("[] in Demo>>total")
                })),
            "the block declined in the body's own scope: {:?}",
            plans.iter().map(|plan| &plan.declined).collect::<Vec<_>>()
        );
    }
}
