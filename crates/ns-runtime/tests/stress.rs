//! The stress mode: a debug build that moves every object and every reified
//! frame at the run loop's safepoints, so that a handle the tracer does not
//! know about panics at its first read (`docs/design/memory.md`, "Checking
//! the roots before a collector exists: the stress mode").
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
        | u ::= a. |
      ) (
        public u = ( ^u )
      ) : ()
      public class Cell = Base x: 5 (
        | v ::= 1. |
      ) (
        public v = ( ^v )
      ) : ()
      public class Late = self base (
      ) (
        public w = ( ^9 )
      ) : ()
      base = ( ^Object )
      public run = (
        | sum n cond body |
        sum := 0.
        #(1 2 3 4) do: [:e | sum := sum + e].
        n := 0.
        cond := [n < 3].
        body := [n := n + 1. sum := sum + n].
        cond whileTrue: body.
        sum := sum + Cell new u + Cell new v.
        sum := sum + Late new w.
        ^sum
      )
    ) : ()
"};

/// `run`'s answer, how many times everything moved, and how many safepoints
/// the stress mode took.
fn run_every_continuation(stress: bool) -> (i64, u32, usize) {
    let (mut interpreter, class) = load(EVERY_CONTINUATION);
    interpreter.set_stress_moves(stress);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let sum = expect_ok(
        interpreter.send(&instance, Symbol::intern("run"), &[]),
        "run",
    );
    let sum = interpreter.enter(|mutator| match mutator.fetch(&sum) {
        Value::SmallInteger(sum) => sum,
        _ => panic!("`run` answers an integer"),
    });
    (sum, interpreter.moves(), interpreter.stress_safepoints())
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
