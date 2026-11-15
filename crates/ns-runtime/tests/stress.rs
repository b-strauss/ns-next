//! The stress mode: a debug build that moves every object and every reified
//! frame at the run loop's safepoints, so that a handle the tracer does not
//! know about panics at its first read (`docs/design/memory.md`, "The move
//! stress mode").
#![cfg(debug_assertions)]

use indoc::indoc;
use ns_runtime::{HostResult, HostSignal, Interpreter, Lowering, Root, Symbol, ValueKind};

fn load(source: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
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

/// Every continuation that holds a handle is on the stack while the block or
/// body it waits for runs, and a move happens at every safepoint of a heap
/// this small: a `do:` loop's receiver, block and elements, a native
/// `whileTrue:`'s condition and body, an instantiation's instance, levels
/// and arguments on the way up and down, and a nested class's enclosing
/// object and declaring class.
const EVERY_CONTINUATION: &str = indoc! {"
    class S = Object new (
    ) (
      public class Base x: a = Object new (
        | uValue ::= a. |
      ) (
        public u = ( ^uValue )
      ) : ()
      public class Cell = Base x: 5 (
        | vValue ::= 1. |
      ) (
        public v = ( ^vValue )
      ) : ()
      public class Late = self base (
      ) (
        public w = ( ^9 )
      ) : ()
      base = ( ^Object )
      public run = (
        | sum n cond body |
        sum:: 0.
        {1. 2. 3. 4} do: [:e | sum:: sum + e].
        n:: 0.
        cond:: [n < 3].
        body:: [n:: n + 1. sum:: sum + n].
        cond whileTrue: body.
        sum:: sum + Cell new u + Cell new v.
        sum:: sum + Late new w.
        ^sum
      )
    ) : ()
"};

/// Instantiates `class` and answers what its `run` answers.
fn run_sum(interpreter: &mut Interpreter, class: &Root) -> i64 {
    let instance = expect_ok(interpreter.instantiate(class, &[]), "instantiate");
    let sum = expect_ok(
        interpreter.send(&instance, Symbol::intern("run"), &[]),
        "run",
    );
    interpreter.enter(|mutator| match mutator.fetch(&sum).kind() {
        ValueKind::SmallInteger(sum) => sum,
        _ => panic!("run answers a small integer"),
    })
}

/// `run`'s answer, how many times everything moved, and how many safepoints
/// the stress mode took. The collect and the scavenge stress modes are
/// turned off regardless of `NS_NEXT_STRESS_COLLECT` and
/// `NS_NEXT_STRESS_SCAVENGE`: a collection and a scavenge both move objects
/// (a collection compacts against the mark bits, a scavenge copies the
/// nursery's survivors, rather than merely shifting, but a debug build bumps
/// the same move epoch for each, since a handle into the old arrangement is
/// exactly as stale after one as after another), so this comparison -- an
/// exact count of the moves the *move* stress mode alone causes -- needs a
/// baseline neither of the other two can also move.
fn run_every_continuation(stress: bool) -> (i64, u32, usize) {
    let (mut interpreter, class) = load(EVERY_CONTINUATION);
    interpreter.set_stress_moves(stress);
    interpreter.set_stress_collect(false);
    interpreter.set_stress_scavenge(false);
    // Counted from here: the bootstrap runs shipped code of its own (the
    // platform's boot), which a stress mode set in the environment already
    // moved under before any switch above was thrown.
    let moves_before = interpreter.moves();
    let safepoints_before = interpreter.stress_safepoints();
    let sum = run_sum(&mut interpreter, &class);
    (
        sum,
        interpreter.moves() - moves_before,
        interpreter.stress_safepoints() - safepoints_before,
    )
}

#[test]
fn every_continuation_survives_a_move_at_every_safepoint() {
    assert_eq!(run_every_continuation(false), (31, 0, 0), "the control");
    let (sum, moves, safepoints) = run_every_continuation(true);
    assert_eq!(sum, 31);
    assert!(
        moves > 20,
        "only {moves} moves: the stress mode did not run"
    );
    // A heap this far within the budget moves at every safepoint, so every
    // continuation is moved under, not only some.
    assert_eq!(
        moves as usize, safepoints,
        "{safepoints} safepoints but {moves} moves: the heap outgrew the stress budget"
    );
}

/// The control above turns the collect stress mode off, on purpose, to
/// isolate the move stress mode's exact move count from an ambient
/// `NS_NEXT_STRESS_COLLECT=1` -- so it never runs this fixture, the suite's
/// richest continuation shape, under the collect stress mode with its exact
/// answer checked. This does: a premature reclamation that turned 31 into
/// something else would have nowhere else to be caught, since the other
/// collect stress coverage (`collector.rs`) uses a simpler fixture.
#[test]
fn every_continuation_survives_a_collection_at_every_safepoint() {
    let (mut interpreter, class) = load(EVERY_CONTINUATION);
    interpreter.set_stress_collect(true);
    let sum = run_sum(&mut interpreter, &class);
    assert_eq!(sum, 31);
    assert!(
        interpreter.stress_collections() > 0,
        "the collect stress mode never collected"
    );
}

/// The scavenge stress mode verifies the remembered set and scavenges at
/// every safepoint; the program's answer must not change.
#[test]
fn every_continuation_survives_a_scavenge_at_every_safepoint() {
    let (mut interpreter, class) = load(EVERY_CONTINUATION);
    interpreter.set_stress_scavenge(true);
    let sum = run_sum(&mut interpreter, &class);
    assert_eq!(sum, 31);
    assert!(
        interpreter.stress_scavenges() > 0,
        "the scavenge stress mode never scavenged"
    );
}

/// The cache stress mode vacates every cache at every safepoint, so filling,
/// transitions and invalidation all run under a program with every kind of
/// continuation; its answer is the one without the mode.
#[test]
fn the_cache_stress_mode_invalidates_at_every_safepoint_and_changes_no_answer() {
    let (mut calm_interpreter, calm_class) = load(EVERY_CONTINUATION);
    let calm = run_sum(&mut calm_interpreter, &calm_class);
    let (mut interpreter, class) = load(EVERY_CONTINUATION);
    interpreter.set_stress_caches(true);
    let stressed = run_sum(&mut interpreter, &class);
    assert_eq!(calm, stressed);
    assert_eq!(calm, 31);
    assert!(
        interpreter.cache_invalidations() > 10,
        "an invalidation at every safepoint of the run"
    );
}

// ---------- The suspend stress mode ----------
//
// Each program runs twice, plain and with every activation of the run
// suspended into its frame at the safepoints the mode's schedule picks,
// and must end the same way both times (`docs/design/execution.md`,
// "Activations").

/// How a run of `source`'s `run` ended, as text -- its answer, or the
/// error it ended in -- and how often the suspend stress mode fired.
fn run_suspended(source: &str, lowering: Lowering, suspend: bool) -> (String, usize) {
    let (outcome, suspensions, _) = run_suspended_with(source, lowering, suspend, None);
    (outcome, suspensions)
}

/// `run_suspended`, and what the JIT did meanwhile, with the JIT stress
/// mode -- every body compiled at its first entry -- set as `jit_stress`
/// says, or as the environment does for `None`.
fn run_suspended_with(
    source: &str,
    lowering: Lowering,
    suspend: bool,
    jit_stress: Option<bool>,
) -> (String, usize, ns_runtime::JitStatistics) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    if let Some(jit_stress) = jit_stress {
        interpreter.set_jit_stress(jit_stress);
    }
    interpreter.set_stress_suspend(suspend);
    let jit_before = interpreter.jit_statistics();
    // Counted from here, as `run_every_continuation` counts its moves.
    let suspensions_before = interpreter.stress_suspensions();
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let outcome = match interpreter.send(&instance, Symbol::intern("run"), &[]) {
        Ok(answer) => interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
            ValueKind::SmallInteger(answer) => format!("answered {answer}"),
            _ => "answered something other than a small integer".to_string(),
        }),
        Err(HostSignal::Return) => "a non-local return escaped".to_string(),
        Err(HostSignal::Error(error)) => format!("failed: {}", error.message),
    };
    let jit_after = interpreter.jit_statistics();
    let mut jit = jit_after;
    jit.compiled_bodies = jit_after.compiled_bodies - jit_before.compiled_bodies;
    jit.materialized_frames = jit_after.materialized_frames - jit_before.materialized_frames;
    (
        outcome,
        interpreter.stress_suspensions() - suspensions_before,
        jit,
    )
}

/// Runs `source` plain and under the suspend stress mode, and checks that
/// both runs end in `expected` and that the mode fired.
fn assert_runs_on_from_suspended_frames(source: &str, lowering: Lowering, expected: &str) {
    let (plain, plain_suspensions) = run_suspended(source, lowering, false);
    assert_eq!(plain, expected, "the control");
    assert_eq!(plain_suspensions, 0);
    let (suspended, suspensions) = run_suspended(source, lowering, true);
    assert_eq!(suspended, plain);
    assert!(suspensions > 0, "the suspend stress mode never suspended");
}

/// Two thousand activations deep, every one of them suspended and resumed
/// on the way down and summing on the way back up. Each runs a short loop
/// first, whose back edge is a safepoint, so that the suspend mode stops a
/// recursion machine code runs, compiled at its first entry or its second
/// alike: machine code polls at back edges and grows the stack itself
/// (`jit::leaves::grow_stack`), and a recursion without a loop would reach
/// no safepoint of the run loop until it returned.
const DEEP_RECURSION: &str = indoc! {"
    class Deep = Object new (
    ) (
      sumTo: count = (
        | index ::= 0. |
        [index < 2] whileTrue: [index:: index + 1].
        ^count = 0 ifTrue: [0] ifFalse: [count + (self sumTo: count - 1)]
      )
      public run = (
        ^self sumTo: 2000
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    fn a_deep_recursion_runs_on_from_suspended_frames(lowering) {
        assert_runs_on_from_suspended_frames(DEEP_RECURSION, lowering, "answered 2001000");
    }
}

ns_runtime::for_each_lowering! {
    /// The JIT stress mode and the suspend mode together: every body
    /// compiled at its first entry, so that every frame of the recursion is
    /// one machine code pushed, and every one the suspend mode stops at a
    /// loop's back edge is first materialized by the exit there
    /// (`materialized_frames`), then suspended into its frame and resumed.
    fn a_deep_recursion_in_machine_code_runs_on_from_suspended_frames(lowering) {
        let (plain, _, _) = run_suspended_with(DEEP_RECURSION, lowering, false, Some(true));
        assert_eq!(plain, "answered 2001000", "the control");
        let (suspended, suspensions, jit) =
            run_suspended_with(DEEP_RECURSION, lowering, true, Some(true));
        assert_eq!(suspended, plain);
        assert!(suspensions > 0, "the suspend stress mode never suspended");
        // Where machine code ran at all: not without the JIT, nor in a
        // build without a JIT.
        if jit.compiled_bodies > 0 {
            assert!(
                jit.materialized_frames > 0,
                "no frame machine code pushed was materialized to be suspended"
            );
        }
    }
}

/// Blocks that capture and write a variable of their method, a native
/// `whileTrue:` over blocks no lowering inlines, `do:` with a block and
/// `do:` with an object that is no block: each continuation is taken
/// through a suspended sender.
const BLOCKS_AND_LOOPS: &str = indoc! {"
    class Loops = Object new (
    ) (
      class Collector = Object new (
        | public total ::= 0. |
      ) (
        public value: element = (
          total:: total + element.
        )
      ) : ()
      public run = (
        | sum count condition body adder elements collector |
        sum:: 0.
        adder:: [:each | sum:: sum + each].
        1 to: 1000 do: adder.
        count:: 0.
        condition:: [count < 1000].
        body:: [count:: count + 1. sum:: sum + count].
        condition whileTrue: body.
        elements:: Array new: 1000.
        1 to: 1000 do: [:index | elements at: index put: index].
        elements do: [:each | sum:: sum + each].
        collector:: Collector new.
        elements do: collector.
        ^sum + collector total
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    fn blocks_loops_and_do_run_on_from_suspended_frames(lowering) {
        assert_runs_on_from_suspended_frames(BLOCKS_AND_LOOPS, lowering, "answered 2002000");
    }
}

/// A superclass clause that runs long enough to pass several safepoints,
/// so that its activation -- whose continuation creates the class -- is
/// suspended and resumed while the creation is in progress; the class is
/// still created once, and a second access answers the same class.
const SLOW_SUPERCLASS_CLAUSE: &str = indoc! {"
    class Outer = Object new (
    ) (
      public class Local = self slowSuperclass (
      ) (
        public answer = ( ^7 )
      ) : ()
      slowSuperclass = (
        | sum |
        sum:: 0.
        1 to: 3000 do: [:each | sum:: sum + each].
        ^Object
      )
      public run = (
        | first second |
        first:: Local.
        second:: Local.
        ^first == second ifTrue: [first new answer] ifFalse: [0]
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    fn a_nested_class_created_across_a_suspension_is_cached_once(lowering) {
        assert_runs_on_from_suspended_frames(SLOW_SUPERCLASS_CLAUSE, lowering, "answered 7");
    }
}

/// Simultaneous slots whose first initializer forces the second's future,
/// whose computation loops past several safepoints: the instruction that
/// forced it is run again in a sender that was suspended meanwhile.
const SLOW_FUTURE: &str = indoc! {"
    class Holder = Object new (
    ) (
      class Probe = Object new (
        ||
          first = second + 1.
          second = self slowSum.
        ||
      ) (
        slowSum = (
          | sum |
          sum:: 0.
          1 to: 3000 do: [:each | sum:: sum + each].
          ^sum
        )
        public answer = ( ^first + second )
      ) : ()
      public run = (
        ^Probe new answer
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    fn a_future_forced_across_a_suspension_runs_its_instruction_again(lowering) {
        assert_runs_on_from_suspended_frames(SLOW_FUTURE, lowering, "answered 9003001");
    }
}

// ---------- Unwinding through suspended frames ----------
//
// The handler search, `return:`, `resume:`, `retry`, `ensure:` and a block's
// `^` each reach activations the suspend stress mode moved into their
// frames: a loop that passes many safepoints (`spin`) runs above them.

/// An `Error` signalled fifty activations above its `on:do:`, with a loop
/// that passes safepoints both before it is signalled and in its handler:
/// the search walks through suspended senders to the handler, and the
/// handler's `return:` unwinds into the suspended `on:do:`.
const HANDLER_BELOW_SUSPENDED_FRAMES: &str = indoc! {"
    class Handled = Object new (
    ) (
      spin = (
        | sum |
        sum:: 0.
        1 to: 3000 do: [:each | sum:: sum + each].
        ^sum
      )
      descend: count = (
        count = 0 ifTrue: [
          self spin.
          ^(Error signal: 'deep') + 1
        ].
        ^self descend: count - 1
      )
      public run = (
        ^([self descend: 50] on: Error do: [:e | self spin. e return: 42]) + 1
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    fn an_exception_signalled_above_suspended_frames_finds_its_handler(lowering) {
        assert_runs_on_from_suspended_frames(HANDLER_BELOW_SUSPENDED_FRAMES, lowering, "answered 43");
    }
}

/// `retry` from a handler that passes safepoints, its `on:do:` suspended
/// meanwhile: the protected block runs again until it lets the exception
/// through.
const RETRY_A_SUSPENDED_ON_DO: &str = indoc! {"
    class Retrying = Object new (
      | attempts ::= 0. |
    ) (
      spin = (
        | sum |
        sum:: 0.
        1 to: 3000 do: [:each | sum:: sum + each].
        ^sum
      )
      public run = (
        ^[
          attempts:: attempts + 1.
          self spin.
          attempts < 3 ifTrue: [Error signal].
          attempts * 10
        ] on: Error do: [:e | self spin. e retry]
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    fn retry_restarts_a_suspended_on_do_activation(lowering) {
        assert_runs_on_from_suspended_frames(RETRY_A_SUSPENDED_ON_DO, lowering, "answered 30");
    }
}

/// Three nested `ensure:` and an `ifCurtailed:`, a loop that passes
/// safepoints inside the innermost, then a `^` out of a block past all of
/// them: each block runs once, innermost first, which the digits of `log`
/// record, and each passes safepoints of its own.
const ENSURE_BLOCKS_OF_SUSPENDED_ACTIVATIONS: &str = indoc! {"
    class Unwinding = Object new (
      | log ::= 0. |
    ) (
      spin = (
        | sum |
        sum:: 0.
        1 to: 3000 do: [:each | sum:: sum + each].
        ^sum
      )
      note: digit = (
        self spin.
        log:: log * 10 + digit.
      )
      escape = (
        [
          [
            [
              [
                self spin.
                ^5
              ] ifCurtailed: [self note: 1]
            ] ensure: [self note: 2]
          ] ensure: [self note: 3]
        ] ensure: [self note: 4].
        ^0
      )
      public run = (
        ^self escape + (log * 10)
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    fn an_unwind_runs_the_ensure_blocks_of_suspended_activations_in_order(lowering) {
        assert_runs_on_from_suspended_frames(
            ENSURE_BLOCKS_OF_SUSPENDED_ACTIVATIONS,
            lowering,
            "answered 12345",
        );
    }
}

/// A block's `^` while its home is suspended below the method that runs
/// the block, which passes safepoints: the home is still running, so the
/// return is caught there rather than being a `cannotReturn`.
const RETURN_INTO_A_SUSPENDED_HOME: &str = indoc! {"
    class Finder = Object new (
    ) (
      each: block = (
        1 to: 3000 do: [:index | block value: index].
      )
      find = (
        self each: [:value | value = 2500 ifTrue: [^value]].
        ^0
      )
      public run = (
        ^self find + 1
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    fn a_return_into_a_suspended_home_is_caught_there(lowering) {
        assert_runs_on_from_suspended_frames(RETURN_INTO_A_SUSPENDED_HOME, lowering, "answered 2501");
    }
}

/// `return:` to an `on:do:` and `resume:` to a `signal` that are suspended
/// while the handler passes safepoints, twenty activations above the
/// `on:do:`.
const RETURN_AND_RESUME_ON_SUSPENDED_ACTIVATIONS: &str = indoc! {"
    class Resuming = Object new (
    ) (
      spin = (
        | sum |
        sum:: 0.
        1 to: 3000 do: [:each | sum:: sum + each].
        ^sum
      )
      descend: count then: block = (
        count = 0 ifTrue: [
          self spin.
          ^block value
        ].
        ^self descend: count - 1 then: block
      )
      returned = (
        ^[
          self descend: 20 then: [
            Error signal.
            1
          ]
        ] on: Error do: [:e | self spin. e return: 5]
      )
      resumed = (
        ^[self descend: 20 then: [(Exception signal) + 1]] on: Exception do: [:e |
          self spin.
          e resume: 7
        ]
      )
      public run = (
        ^self returned * 100 + self resumed
      )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    fn return_value_and_resume_on_a_suspended_activation(lowering) {
        assert_runs_on_from_suspended_frames(
            RETURN_AND_RESUME_ON_SUSPENDED_ACTIVATIONS,
            lowering,
            "answered 508",
        );
    }
}

/// The depth limit's fixture: a recursion to the limit, which records how
/// deep it got; an error raised at that depth and one level further; a
/// handler recursing past the reserve, which records how deep that got;
/// and errors raised short of where the reserve ends and at it.
const DEPTH_LIMIT: &str = indoc! {"
    class Deep = Object new (
      |
        public deepest ::= 0.
        public reserveDeepest ::= 0.
      |
    ) (
      public probe: count = (
        deepest:: count.
        ^self probe: count + 1
      )
      reserveProbe: count = (
        reserveDeepest:: count.
        ^self reserveProbe: count + 1
      )
      public descend: count to: bottom = (
        count = bottom ifTrue: [^{1} at: 5].
        ^self descend: count + 1 to: bottom
      )
      public measureTheReserve = (
        ^[[self probe: 1] on: Error do: [:e | self reserveProbe: 1]] on: Error do: [:e | 7]
      )
      public failInTheReserveAt: bottom = (
        ^[[self probe: 1] on: Error do: [:e | self descend: 1 to: bottom]] on: Error do: [:e | 7]
      )
    ) : ()
"};

/// How each step of the depth limit's fixture ended -- its answer or its
/// error, and each depth it recorded -- and how often the suspend stress
/// mode fired.
fn depth_limit_outcomes(lowering: Lowering, suspend: bool) -> (Vec<String>, usize) {
    let module = ns_syntax::parse(DEPTH_LIMIT).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        DEPTH_LIMIT.into(),
        ns_runtime::SourceOrigin::File("deep.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    interpreter.set_stress_suspend(suspend);
    let suspensions_before = interpreter.stress_suspensions();
    let deep = expect_ok(interpreter.instantiate(&class, &[]), "Deep new");
    let describe = |interpreter: &mut Interpreter, result: HostResult| match result {
        Ok(answer) => interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
            ValueKind::SmallInteger(answer) => format!("answered {answer}"),
            _ => "answered something other than a small integer".to_string(),
        }),
        Err(HostSignal::Return) => "a non-local return escaped".to_string(),
        Err(HostSignal::Error(error)) => format!("failed: {}", error.message),
    };
    let integer = |interpreter: &mut Interpreter, selector: &str| {
        let value = expect_ok(
            interpreter.send(&deep, Symbol::intern(selector), &[]),
            selector,
        );
        interpreter.enter(|mutator| match mutator.fetch(&value).kind() {
            ValueKind::SmallInteger(integer) => integer,
            _ => panic!("{selector} answers a small integer"),
        })
    };
    let send_integers = |interpreter: &mut Interpreter, selector: &str, arguments: &[i64]| {
        let arguments: Vec<Root> = arguments
            .iter()
            .map(|&argument| interpreter.stash(ns_runtime::Value::small_integer(argument)))
            .collect();
        interpreter.send(&deep, Symbol::intern(selector), &arguments)
    };
    let mut outcomes = Vec::new();
    let result = send_integers(&mut interpreter, "probe:", &[1]);
    outcomes.push(describe(&mut interpreter, result));
    let deepest = integer(&mut interpreter, "deepest");
    outcomes.push(format!("deepest {deepest}"));
    for bottom in [deepest, deepest + 1] {
        let result = send_integers(&mut interpreter, "descend:to:", &[1, bottom]);
        outcomes.push(describe(&mut interpreter, result));
    }
    let result = interpreter.send(&deep, Symbol::intern("measureTheReserve"), &[]);
    outcomes.push(describe(&mut interpreter, result));
    let reserve_deepest = integer(&mut interpreter, "reserveDeepest");
    outcomes.push(format!("reserve deepest {reserve_deepest}"));
    for bottom in [reserve_deepest - 500, reserve_deepest - 2, reserve_deepest] {
        let result = send_integers(&mut interpreter, "failInTheReserveAt:", &[bottom]);
        outcomes.push(describe(&mut interpreter, result));
    }
    (
        outcomes,
        interpreter.stress_suspensions() - suspensions_before,
    )
}

ns_runtime::for_each_lowering! {
    /// The suspended activations of a deep recursion count against the
    /// depth limit as running ones do: the limit is met at the same depth,
    /// an error raised there is reported from the reserve as itself, and
    /// the reserve ends at the same depth and reports the same errors.
    fn the_depth_limit_counts_suspended_activations(lowering) {
        const INDEX_ERROR: &str = "failed: index 5 out of range: valid indices are 1..1 (size 1)";
        const DEPTH_ERROR: &str = "failed: stack overflow: recursion too deep";
        let (plain, plain_suspensions) = depth_limit_outcomes(lowering, false);
        assert_eq!(plain_suspensions, 0);
        let expected = [
            (0, DEPTH_ERROR),
            (2, INDEX_ERROR),
            (3, DEPTH_ERROR),
            (4, DEPTH_ERROR),
            (6, "answered 7"),
            (7, INDEX_ERROR),
            (8, INDEX_ERROR),
        ];
        for (position, outcome) in expected {
            assert_eq!(plain[position], outcome, "the control, step {position}");
        }
        let (suspended, suspensions) = depth_limit_outcomes(lowering, true);
        assert_eq!(suspended, plain);
        assert!(suspensions > 0, "the suspend stress mode never suspended");
    }
}

// The pause stress mode: at the safepoints its schedule picks, the run
// stops as for a debugger's pause, its whole stack and every value one
// level deep read, and goes on (`docs/design/debugger.md`, "The pause
// stress mode").

/// Closures that capture and write their method's locals, an array of
/// them, and a recursion a thousand activations deep, each running a short
/// loop whose back edge is a safepoint, so that the stops read deep stacks
/// of blocks, arrays and instances.
const PAUSED_PROGRAM: &str = indoc! {"
    class Paused = Object new (
    ) (
      class Holder = Object new (
        | public blocks = Array new: 4. public total ::= 0. |
      ) (
      ) : ()
      depth: count holder: holder = (
        | index ::= 0. |
        [index < 2] whileTrue: [index:: index + 1].
        count = 0 ifTrue: [^0].
        holder total: holder total + 1.
        ^count + (self depth: count - 1 holder: holder)
      )
      public run = (
        | sum holder |
        sum:: 0.
        holder:: Holder new.
        1 to: 4 do: [:index | holder blocks at: index put: [:each | sum:: sum + (each * index)]].
        1 to: 100 do: [:each | holder blocks do: [:block | block value: each]].
        sum:: sum + (self depth: 1000 holder: holder).
        ^sum + holder total
      )
    ) : ()
"};

/// How a run of `source`'s `run` ended, and how often the pause stress
/// mode stopped it.
fn run_paused(source: &str, lowering: Lowering, pause: bool) -> (String, usize) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    interpreter.set_stress_pause(pause);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let outcome = match interpreter.send(&instance, Symbol::intern("run"), &[]) {
        Ok(answer) => interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
            ValueKind::SmallInteger(answer) => format!("answered {answer}"),
            _ => "answered something other than a small integer".to_string(),
        }),
        Err(HostSignal::Return) => "a non-local return escaped".to_string(),
        Err(HostSignal::Error(error)) => format!("failed: {}", error.message),
    };
    (outcome, interpreter.stress_pauses())
}

ns_runtime::for_each_lowering! {
    fn the_pause_stress_mode_keeps_outcomes(lowering) {
        let (plain, plain_pauses) = run_paused(PAUSED_PROGRAM, lowering, false);
        assert_eq!(plain, "answered 552000", "the control");
        assert_eq!(plain_pauses, 0);
        let (paused, pauses) = run_paused(PAUSED_PROGRAM, lowering, true);
        assert_eq!(paused, plain);
        assert!(pauses > 0, "the pause stress mode never stopped");
    }
}

// The breakpoint stress mode: every statement start of every body found
// running at a safepoint patched with a breakpoint that never stops, each hit gone past
// through the run loop's arm (`docs/design/debugger.md`, "The breakpoint
// stress mode").

/// A loop whose blocks a callee runs, a non-local return out of a `do:`,
/// an exception its handler returns from and a recursion, each run often
/// enough to be compiled, so that the patched statement starts stand in
/// inlined blocks, closures, handlers and unwound frames, in machine code
/// and out of it.
const BREAKPOINTED_PROGRAM: &str = indoc! {"
    class Breakpointed = Object new (
    ) (
      class Counter = Object new (
        | public total ::= 0. |
      ) (
        public add: amount = (
          total:: total + amount
        )
      ) : ()
      firstAbove: limit in: items = (
        items do: [:each | each > limit ifTrue: [^each]].
        ^0
      )
      guarded: index = (
        ^[
          index \\\\ 3 = 0 ifTrue: [Error new signal: 'three'].
          index
        ]
          on: Error
          do: [:error | error return: 100]
      )
      depth: count = (
        count = 0 ifTrue: [^0].
        ^1 + (self depth: count - 1)
      )
      public run = (
        | counter items |
        counter:: Counter new.
        items:: {3. 9. 27. 81}.
        1 to: 200 do: [:index |
          counter add: (self firstAbove: index \\\\ 50 in: items).
          counter add: (self guarded: index)].
        counter add: (self depth: 100).
        ^counter total
      )
    ) : ()
"};

/// How a run of `source`'s `run` ended under `lowering`, the JIT on or
/// off, and how many breakpoints the breakpoint stress mode went past.
fn run_breakpointed(
    source: &str,
    lowering: Lowering,
    jit: bool,
    breakpoints: bool,
) -> (String, usize) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    interpreter.set_jit_enabled(jit);
    interpreter.set_stress_breakpoints(breakpoints);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let outcome = match interpreter.send(&instance, Symbol::intern("run"), &[]) {
        Ok(answer) => interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
            ValueKind::SmallInteger(answer) => format!("answered {answer}"),
            _ => "answered something other than a small integer".to_string(),
        }),
        Err(HostSignal::Return) => "a non-local return escaped".to_string(),
        Err(HostSignal::Error(error)) => format!("failed: {}", error.message),
    };
    (outcome, interpreter.stress_breakpoint_hits())
}

ns_runtime::for_each_lowering! {
    fn the_breakpoint_stress_mode_keeps_outcomes(lowering) {
        for jit in [true, false] {
            let (plain, plain_hits) = run_breakpointed(BREAKPOINTED_PROGRAM, lowering, jit, false);
            assert_eq!(plain, "answered 29815", "the control, the JIT on: {jit}");
            assert_eq!(plain_hits, 0);
            let (breakpointed, hits) =
                run_breakpointed(BREAKPOINTED_PROGRAM, lowering, jit, true);
            assert_eq!(breakpointed, plain, "the JIT on: {jit}");
            // Each of the loop's 200 passes goes past its own two
            // statements and the one of each of its two `add:`s at least.
            assert!(
                hits >= 800,
                "the breakpoint stress mode went past {hits} breakpoints, the JIT on: {jit}"
            );
        }
    }
}

// The step stress mode: at the safepoints its schedule picks, the run
// stops as for a debugger's pause and takes a burst of steps -- into,
// over and out in turn, the shipped filter switched every third --
// reading the stack and its values at every stop
// (`docs/design/debugger.md`, "The step stress mode").

/// `BREAKPOINTED_PROGRAM`'s work and answer, its 200 rounds a recursion
/// rather than a loop's block: a step out of a block leaves the whole send
/// that took it (decision 17), so that the step stress mode's steps out
/// of a loop's block run the rest of the loop, while those out of a round
/// here leave one frame.
const STEPPED_PROGRAM: &str = indoc! {"
    class Stepped = Object new (
    ) (
      class Counter = Object new (
        | public total ::= 0. |
      ) (
        public add: amount = (
          total:: total + amount
        )
      ) : ()
      firstAbove: limit in: items = (
        items do: [:each | each > limit ifTrue: [^each]].
        ^0
      )
      guarded: index = (
        ^[
          index \\\\ 3 = 0 ifTrue: [Error new signal: 'three'].
          index
        ]
          on: Error
          do: [:error | error return: 100]
      )
      depth: count = (
        count = 0 ifTrue: [^0].
        ^1 + (self depth: count - 1)
      )
      rounds: count into: counter over: items = (
        count = 0 ifTrue: [^0].
        self rounds: count - 1 into: counter over: items.
        counter add: (self firstAbove: count \\\\ 50 in: items).
        counter add: (self guarded: count).
        ^count
      )
      public run = (
        | counter items |
        counter:: Counter new.
        items:: {3. 9. 27. 81}.
        self rounds: 200 into: counter over: items.
        counter add: (self depth: 100).
        ^counter total
      )
    ) : ()
"};

/// How a run of `source`'s `run` ended under `lowering`, the JIT on or
/// off, and how many of the step stress mode's steps stopped and how many
/// it ended itself.
fn run_stepped(source: &str, lowering: Lowering, jit: bool, step: bool) -> (String, usize, usize) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    interpreter.set_jit_enabled(jit);
    interpreter.set_stress_step(step);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let outcome = match interpreter.send(&instance, Symbol::intern("run"), &[]) {
        Ok(answer) => interpreter.enter(|mutator| match mutator.fetch(&answer).kind() {
            ValueKind::SmallInteger(answer) => format!("answered {answer}"),
            _ => "answered something other than a small integer".to_string(),
        }),
        Err(HostSignal::Return) => "a non-local return escaped".to_string(),
        Err(HostSignal::Error(error)) => format!("failed: {}", error.message),
    };
    (
        outcome,
        interpreter.stress_steps(),
        interpreter.stress_steps_ended(),
    )
}

ns_runtime::for_each_lowering! {
    fn the_step_stress_mode_keeps_outcomes(lowering) {
        for jit in [true, false] {
            let (plain, plain_steps, plain_ended) =
                run_stepped(STEPPED_PROGRAM, lowering, jit, false);
            assert_eq!(plain, "answered 29815", "the control, the JIT on: {jit}");
            assert_eq!((plain_steps, plain_ended), (0, 0));
            let (stepped, steps, ended) = run_stepped(STEPPED_PROGRAM, lowering, jit, true);
            assert_eq!(stepped, plain, "the JIT on: {jit}");
            // Many whole bursts. The steps the mode ends itself are steps
            // out of a long send -- the recursion's outer rounds, `run`,
            // which returns to the host -- that the schedule's period
            // outlasts. The test runs under every mode of the `stress`
            // gate: measured under each of the fifteen and with the JIT
            // off, the steps that stopped were at least 475 (the JIT
            // stress mode's optimized run with the JIT) and at least 11
            // times those ended (the suspend stress mode's reference run),
            // since the grace's floor of safepoints keeps it from
            // shrinking with a heap the collect and scavenge stress modes
            // keep small. The bounds lie far below both.
            let burst = Interpreter::stress_step_burst();
            assert!(
                steps >= 6 * burst,
                "the step stress mode's steps stopped {steps} times, the JIT on: {jit}"
            );
            assert!(
                steps >= 4 * ended,
                "{steps} steps stopped and {ended} were ended, the JIT on: {jit}"
            );
        }
    }
}
