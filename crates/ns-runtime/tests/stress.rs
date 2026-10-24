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
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        source.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    interpreter.set_stress_suspend(suspend);
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
    (
        outcome,
        interpreter.stress_suspensions() - suspensions_before,
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
/// on the way down and summing on the way back up.
const DEEP_RECURSION: &str = indoc! {"
    class Deep = Object new (
    ) (
      sumTo: count = (
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
