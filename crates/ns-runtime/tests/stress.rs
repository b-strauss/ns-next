//! The stress mode: a debug build that moves every object and every reified
//! frame at the run loop's safepoints, so that a handle the tracer does not
//! know about panics at its first read (`docs/design/memory.md`, "The move
//! stress mode").
#![cfg(debug_assertions)]

use indoc::indoc;
use ns_runtime::{HostResult, HostSignal, Interpreter, Root, Symbol, Value};

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
    Newspeak3
    'T'
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
        #(1 2 3 4) do: [:e | sum:: sum + e].
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
    interpreter.enter(|mutator| match mutator.fetch(&sum) {
        Value::SmallInteger(sum) => sum,
        _ => panic!("run answers a small integer"),
    })
}

/// `run`'s answer, how many times everything moved, and how many safepoints
/// the stress mode took. The collect stress mode is turned off regardless of
/// `NS_NEXT_STRESS_COLLECT`: a collection also moves everything (it compacts
/// against the mark bits rather than merely rotating, but a debug build
/// bumps the same move epoch either way, since a handle into the old
/// arrangement is exactly as stale after one as after the other), so this
/// comparison -- an exact count of the moves the *move* stress mode alone
/// causes -- needs a baseline the collect stress mode cannot also move.
fn run_every_continuation(stress: bool) -> (i64, u32, usize) {
    let (mut interpreter, class) = load(EVERY_CONTINUATION);
    interpreter.set_stress_moves(stress);
    interpreter.set_stress_collect(false);
    // Counted from here: the bootstrap runs shipped code of its own (the
    // platform's boot), which a stress mode set in the environment already
    // moved under before either switch above was thrown.
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
