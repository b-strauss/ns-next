//! A host keeps values across runs of the loop only as `Root`s: slots the
//! heap's root table also holds, which a moving collector traces and
//! rewrites (`docs/design/memory.md`, "Host roots").

use indoc::indoc;
use ns_runtime::{HostError, HostResult, HostSignal, Interpreter, Root, Symbol, Value};

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

fn expect_error(interpreter: &mut Interpreter, result: HostResult, what: &str) -> HostError {
    match result {
        Err(HostSignal::Error(error)) => error,
        Ok(root) => {
            let got = interpreter.enter(|mutator| mutator.heap().describe(&mutator.fetch(&root)));
            panic!("{what}: expected a Newspeak error, got {got}")
        }
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
    }
}

const COUNTER: &str = indoc! {"
    class Counter = Object new (
      | n ::= 0. |
    ) (
      public bump: k = ( n:: n + k. ^n )
    ) : ()
"};

#[test]
fn a_host_sends_to_roots_and_gets_roots_back() {
    let (mut interpreter, class) = load(COUNTER);
    let counter = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let bump = Symbol::intern("bump:");
    expect_ok(
        interpreter.send(&counter, bump, &[interpreter.stash(Value::SmallInteger(2))]),
        "bump: 2",
    );
    let total = expect_ok(
        interpreter.send(&counter, bump, &[interpreter.stash(Value::SmallInteger(3))]),
        "bump: 3",
    );
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&total), Value::SmallInteger(5))));
}

#[test]
fn a_newspeak_error_reaches_the_host_with_its_receiver_as_a_root() {
    let (mut interpreter, _class) = load(COUNTER);
    let three = interpreter.stash(Value::SmallInteger(3));
    let result = interpreter.send(&three, Symbol::intern("frobnicate"), &[]);
    let error = expect_error(&mut interpreter, result, "3 frobnicate");
    assert_eq!(error.selector.as_str(), "frobnicate");
    assert!(interpreter
        .enter(|mutator| matches!(mutator.fetch(&error.receiver), Value::SmallInteger(3))));
}

#[test]
fn instantiating_a_root_that_holds_no_class_is_a_newspeak_error() {
    let (mut interpreter, _class) = load(COUNTER);
    let three = interpreter.stash(Value::SmallInteger(3));
    let result = interpreter.instantiate(&three, &[]);
    let error = expect_error(&mut interpreter, result, "instantiate 3");
    assert_eq!(error.message, "cannot instantiate 3: it is not a class");
}

/// A root nothing outside the table holds is swept, so a host that makes
/// many sends and keeps few results does not grow the table without bound
/// -- in a release build too, where nothing moves. A root the host still
/// holds is never swept: the table is what the collector traces, so a live
/// root it dropped would go stale at the first move. `fetch` reads through
/// the root's own slot and cannot tell, so the table's count is what is
/// checked.
#[test]
fn roots_a_host_has_dropped_are_swept() {
    const HELD: usize = 100;
    let (mut interpreter, _class) = load(COUNTER);
    let held: Vec<Root> = (0..HELD as i64)
        .map(|number| interpreter.stash(Value::SmallInteger(-number)))
        .collect();
    for iteration in 0..10_000 {
        let _ = interpreter.stash(Value::SmallInteger(iteration));
    }
    let count = interpreter.enter(|mutator| mutator.heap().host_root_count());
    assert!(
        count >= HELD,
        "the table holds {count} roots while the host still holds {HELD}"
    );
    assert!(
        count < 3 * HELD,
        "the table holds {count} roots after 10,000 dropped ones, with {HELD} held"
    );
    interpreter.enter(|mutator| {
        for (index, root) in held.iter().enumerate() {
            assert!(matches!(mutator.fetch(root), Value::SmallInteger(integer) if integer == -(index as i64)));
        }
    });
}

/// A root belongs to the heap that stashed it: another heap's table does
/// not hold it, so that heap's collector would never trace it.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "handle belongs to a different heap")]
fn fetching_a_root_through_another_interpreter_panics() {
    let (_interpreter, class) = load(COUNTER);
    let (mut other, _other_class) = load(COUNTER);
    other.enter(|mutator| {
        let _ = mutator.fetch(&class);
    });
}

/// Gated like its only caller: a move exists only in a debug build.
#[cfg(debug_assertions)]
const GRAPH: &str = indoc! {"
    class X = Object new (
      | cell ::= nil. counter ::= nil. |
    ) (
      public class Cell = Object new (
        | held ::= 7. |
      ) (
        public v = ( ^held )
        public secret = ( ^outer X secret )
      ) : ()
      public build = (
        | n |
        n:: 40.
        counter:: [n:: n + 1. n].
        cell:: Cell new.
        ^{cell. counter}
      )
      public read = ( ^cell v + counter value )
      public secret = ( ^11 )
      public throughCell = ( ^cell secret )
    ) : (
      public marker = ( ^3 )
    )
"};

/// Moves everything between runs, and checks that it did. The count is
/// taken across the move alone, because under the stress mode the runs
/// around it move as well.
#[cfg(debug_assertions)]
fn move_once(interpreter: &mut Interpreter) {
    let before = interpreter.moves();
    interpreter.move_now();
    assert_eq!(interpreter.moves(), before + 1, "move_now did not move");
}

/// What a host holds as roots survives a move of every object and every
/// reified frame, and so does everything reachable from it: an instance's
/// slots, a nested class and its enclosing object -- which `outer X` reaches
/// through the cell's class object, and which only `X` can answer `secret`
/// for -- a closure and the divorced frame whose variable it writes, a
/// cached metaclass, and the builtin classes every send to an integer
/// reaches.
#[test]
#[cfg(debug_assertions)]
fn a_world_reached_from_roots_survives_moves() {
    let (mut interpreter, class) = load(GRAPH);
    let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
    let built = expect_ok(
        interpreter.send(&instance, Symbol::intern("build"), &[]),
        "build",
    );
    move_once(&mut interpreter);
    let read = expect_ok(
        interpreter.send(&instance, Symbol::intern("read"), &[]),
        "read",
    );
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&read), Value::SmallInteger(48))));
    let secret = expect_ok(
        interpreter.send(&instance, Symbol::intern("throughCell"), &[]),
        "throughCell",
    );
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&secret), Value::SmallInteger(11))));
    move_once(&mut interpreter);
    let read = expect_ok(
        interpreter.send(&instance, Symbol::intern("read"), &[]),
        "read",
    );
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&read), Value::SmallInteger(49))));
    let marker = expect_ok(
        interpreter.send(&class, Symbol::intern("marker"), &[]),
        "marker",
    );
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&marker), Value::SmallInteger(3))));
    move_once(&mut interpreter);
    let marker = expect_ok(
        interpreter.send(&class, Symbol::intern("marker"), &[]),
        "marker",
    );
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&marker), Value::SmallInteger(3))));
    interpreter.enter(|mutator| {
        let Value::Object(built) = mutator.fetch(&built) else {
            panic!("`build` answers an array")
        };
        assert_eq!(mutator.heap().array_length(built), Some(2));
        let cell = mutator.heap().array_at(built, 0).expect("the cell");
        assert_eq!(mutator.heap().describe(&cell), "an instance of Cell");
    });
}
