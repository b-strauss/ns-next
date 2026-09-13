//! A host keeps values across runs of the loop only as `Root`s: slots the
//! heap's root table also holds, which a moving collector traces and
//! rewrites (`docs/design/memory.md`, "Host roots").

use indoc::indoc;
use ns_runtime::{HostError, HostResult, HostSignal, Interpreter, Root, Symbol, Value};

fn load(src: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        src.into(),
        ns_runtime::SourceOrigin::File("test.ns".into()),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class)
}

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(e)) => panic!("{what}: unexpected Newspeak error: {}", e.message),
    }
}

fn expect_error(interp: &mut Interpreter, result: HostResult, what: &str) -> HostError {
    match result {
        Err(HostSignal::Error(e)) => e,
        Ok(root) => {
            let got = interp.enter(|m| m.heap().describe(&m.fetch(&root)));
            panic!("{what}: expected a Newspeak error, got {got}")
        }
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
    }
}

const COUNTER: &str = indoc! {"
    Newspeak3
    'T'
    class Counter = Object new (
      | n ::= 0. |
    ) (
      public bump: k = ( n := n + k. ^n )
    ) : ()
"};

#[test]
fn a_host_sends_to_roots_and_gets_roots_back() {
    let (mut interp, class) = load(COUNTER);
    let counter = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let bump = Symbol::intern("bump:");
    expect_ok(
        interp.send(&counter, bump, &[interp.stash(Value::SmallInt(2))]),
        "bump: 2",
    );
    let n = expect_ok(
        interp.send(&counter, bump, &[interp.stash(Value::SmallInt(3))]),
        "bump: 3",
    );
    assert!(interp.enter(|m| matches!(m.fetch(&n), Value::SmallInt(5))));
}

#[test]
fn a_newspeak_error_reaches_the_host_with_its_receiver_as_a_root() {
    let (mut interp, _class) = load(COUNTER);
    let three = interp.stash(Value::SmallInt(3));
    let result = interp.send(&three, Symbol::intern("frobnicate"), &[]);
    let e = expect_error(&mut interp, result, "3 frobnicate");
    assert_eq!(e.selector.as_str(), "frobnicate");
    assert!(interp.enter(|m| matches!(m.fetch(&e.receiver), Value::SmallInt(3))));
}

#[test]
fn instantiating_a_root_that_holds_no_class_is_a_newspeak_error() {
    let (mut interp, _class) = load(COUNTER);
    let three = interp.stash(Value::SmallInt(3));
    let result = interp.instantiate(&three, &[]);
    let e = expect_error(&mut interp, result, "instantiate 3");
    assert_eq!(e.message, "cannot instantiate 3: it is not a class");
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
    let (mut interp, _class) = load(COUNTER);
    let held: Vec<Root> = (0..HELD as i64)
        .map(|i| interp.stash(Value::SmallInt(-i)))
        .collect();
    for i in 0..10_000 {
        let _ = interp.stash(Value::SmallInt(i));
    }
    let count = interp.enter(|m| m.heap().host_root_count());
    assert!(
        count >= HELD,
        "the table holds {count} roots while the host still holds {HELD}"
    );
    assert!(
        count < 3 * HELD,
        "the table holds {count} roots after 10,000 dropped ones, with {HELD} held"
    );
    interp.enter(|m| {
        for (i, root) in held.iter().enumerate() {
            assert!(matches!(m.fetch(root), Value::SmallInt(v) if v == -(i as i64)));
        }
    });
}

/// A root belongs to the heap that stashed it: another heap's table does
/// not hold it, so that heap's collector would never trace it.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "handle belongs to a different heap")]
fn fetching_a_root_through_another_interpreter_panics() {
    let (_interp, class) = load(COUNTER);
    let (mut other, _other_class) = load(COUNTER);
    let _ = other.enter(|m| m.fetch(&class));
}

/// Gated like its only caller: a move exists only in a debug build.
#[cfg(debug_assertions)]
const GRAPH: &str = indoc! {"
    Newspeak3
    'T'
    class X = Object new (
      | cell ::= nil. counter ::= nil. |
    ) (
      public class Cell = Object new (
        | v ::= 7. |
      ) (
        public v = ( ^v )
        public secret = ( ^outer X secret )
      ) : ()
      public build = (
        | n |
        n := 40.
        counter := [n := n + 1. n].
        cell := Cell new.
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
fn move_once(interp: &mut Interpreter) {
    let before = interp.moves();
    interp.move_now();
    assert_eq!(interp.moves(), before + 1, "move_now did not move");
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
    let (mut interp, class) = load(GRAPH);
    let x = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let built = expect_ok(interp.send(&x, Symbol::intern("build"), &[]), "build");
    move_once(&mut interp);
    let read = expect_ok(interp.send(&x, Symbol::intern("read"), &[]), "read");
    assert!(interp.enter(|m| matches!(m.fetch(&read), Value::SmallInt(48))));
    let secret = expect_ok(
        interp.send(&x, Symbol::intern("throughCell"), &[]),
        "throughCell",
    );
    assert!(interp.enter(|m| matches!(m.fetch(&secret), Value::SmallInt(11))));
    move_once(&mut interp);
    let read = expect_ok(interp.send(&x, Symbol::intern("read"), &[]), "read");
    assert!(interp.enter(|m| matches!(m.fetch(&read), Value::SmallInt(49))));
    let marker = expect_ok(interp.send(&class, Symbol::intern("marker"), &[]), "marker");
    assert!(interp.enter(|m| matches!(m.fetch(&marker), Value::SmallInt(3))));
    move_once(&mut interp);
    let marker = expect_ok(interp.send(&class, Symbol::intern("marker"), &[]), "marker");
    assert!(interp.enter(|m| matches!(m.fetch(&marker), Value::SmallInt(3))));
    interp.enter(|m| {
        let Value::Obj(built) = m.fetch(&built) else {
            panic!("`build` answers an array")
        };
        assert_eq!(m.heap().array_len(built), Some(2));
        let cell = m.heap().array_at(built, 0).expect("the cell");
        assert_eq!(m.heap().describe(&cell), "an instance of Cell");
    });
}

/// A value a host fetched and kept past a move names nothing any more: the
/// debug build refuses the read rather than answer for whatever object took
/// its slot.
///
/// Converted literally per the 2c plan rather than following the "nothing
/// that holds a handle leaves an entry" rule: this test's whole point is the
/// mistake of keeping a fetched value past a move. Task 4's brand makes this
/// a compile error, and replaces this test with a `compile_fail` doctest.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "stale handle")]
fn a_value_kept_past_a_move_is_refused() {
    let (mut interp, class) = load(COUNTER);
    let counter = expect_ok(interp.instantiate(&class, &[]), "instantiate");
    let kept = interp.enter(|m| m.fetch(&counter));
    interp.move_now();
    let _ = interp.enter(|m| m.heap().describe(&kept));
}
