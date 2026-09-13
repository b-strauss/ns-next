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

fn expect_error(interp: &Interpreter, result: HostResult, what: &str) -> HostError {
    match result {
        Err(HostSignal::Error(e)) => e,
        Ok(root) => panic!(
            "{what}: expected a Newspeak error, got {}",
            interp.heap_ref().describe(&interp.fetch(&root))
        ),
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
    assert!(matches!(interp.fetch(&n), Value::SmallInt(5)));
}

#[test]
fn a_newspeak_error_reaches_the_host_with_its_receiver_as_a_root() {
    let (mut interp, _class) = load(COUNTER);
    let three = interp.stash(Value::SmallInt(3));
    let result = interp.send(&three, Symbol::intern("frobnicate"), &[]);
    let e = expect_error(&interp, result, "3 frobnicate");
    assert_eq!(e.selector.as_str(), "frobnicate");
    assert!(matches!(interp.fetch(&e.receiver), Value::SmallInt(3)));
}

#[test]
fn instantiating_a_root_that_holds_no_class_is_a_newspeak_error() {
    let (mut interp, _class) = load(COUNTER);
    let three = interp.stash(Value::SmallInt(3));
    let result = interp.instantiate(&three, &[]);
    let e = expect_error(&interp, result, "instantiate 3");
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
    let (interp, _class) = load(COUNTER);
    let held: Vec<Root> = (0..HELD as i64)
        .map(|i| interp.stash(Value::SmallInt(-i)))
        .collect();
    for i in 0..10_000 {
        let _ = interp.stash(Value::SmallInt(i));
    }
    let count = interp.heap_ref().host_root_count();
    assert!(
        count >= HELD,
        "the table holds {count} roots while the host still holds {HELD}"
    );
    assert!(
        count < 3 * HELD,
        "the table holds {count} roots after 10,000 dropped ones, with {HELD} held"
    );
    for (i, root) in held.iter().enumerate() {
        assert!(matches!(interp.fetch(root), Value::SmallInt(v) if v == -(i as i64)));
    }
}

/// A root belongs to the heap that stashed it: another heap's table does
/// not hold it, so that heap's collector would never trace it.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "handle belongs to a different heap")]
fn fetching_a_root_through_another_interpreter_panics() {
    let (_interp, class) = load(COUNTER);
    let (other, _other_class) = load(COUNTER);
    let _ = other.fetch(&class);
}
