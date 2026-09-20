//! The collector through a real interpreter: what survives a collection,
//! what does not, and what that does to the arenas
//! (`docs/design/memory.md`, "The gates the collector needs").

use indoc::indoc;
use ns_runtime::{Interpreter, Root, Symbol, Value};

/// Loads `source` and answers the interpreter and its module's root class,
/// the way `crates/ns-runtime/tests/heap.rs`, `roots.rs` and `stress.rs`
/// each load one.
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

/// Every test here unwraps through this rather than `Result::unwrap`:
/// `HostSignal` has no `Debug`, on purpose, so `unwrap` does not compile.
fn expect_ok(result: ns_runtime::HostResult) -> ns_runtime::Root {
    match result {
        Ok(value) => value,
        Err(ns_runtime::HostSignal::Error(error)) => {
            panic!("unexpected Newspeak error: {}", error.message)
        }
        Err(ns_runtime::HostSignal::Return) => panic!("unexpected non-local return"),
    }
}

/// A module whose entry allocates a great many short-lived closures and
/// keeps none of them. Collecting between runs must give the arena back.
#[test]
#[cfg(debug_assertions)]
fn a_collection_gives_back_what_a_run_discarded() {
    // `timesRepeat:` is a kernel-library convenience the bare bootstrap does
    // not load, so the loop is written with `whileTrue:`, which is a native
    // primitive (`crates/ns-runtime/src/builtins/primitive.rs`).
    let source = indoc! {"
        Newspeak3
        'T'
        class Churn = Object new () (
          public run: count = (
            | total remaining |
            total:: 0.
            remaining:: count.
            [remaining > 0] whileTrue: [
              | block = [:value | value + 1]. |
              total:: block value: total.
              remaining:: remaining - 1.
            ].
            ^total
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let count = interpreter.stash(Value::SmallInteger(2000));
    let selector = Symbol::intern("run:");

    interpreter.collect_now();
    let before = interpreter.heap_statistics();
    let _ = expect_ok(interpreter.send(&instance, selector, &[count]));
    let after_run = interpreter.heap_statistics();
    interpreter.collect_now();
    let after_collection = interpreter.heap_statistics();

    assert!(
        after_run.live_objects > before.live_objects,
        "the run should have allocated"
    );
    assert!(
        after_collection.live_objects < after_run.live_objects,
        "the collection should have given the discarded closures back"
    );
    assert!(
        after_collection.object_slots <= after_run.object_slots,
        "the buffer must not have grown across a collection"
    );
}

/// §5.1.6: equal symbols are one object. Pruning a symbol nothing holds must
/// not disturb the table's bookkeeping for a different symbol name still
/// being minted: `#kept` is interned twice in one send, after a collection
/// has already pruned `#transient`, and the two occurrences must still be
/// the same object (`docs/design/memory.md`, "Weak references").
#[test]
#[cfg(debug_assertions)]
fn a_symbol_nothing_holds_is_collected_and_minted_again() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Symbols = Object new () (
          public make = ( ^#transient )
          public same = ( ^#kept == #kept )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let make = Symbol::intern("make");
    let same = Symbol::intern("same");

    let discarded = expect_ok(interpreter.send(&instance, make, &[]));
    drop(discarded);
    interpreter.collect_now();

    // The collection above pruned `#transient`, which nothing held past the
    // send that minted it. `#kept` is minted twice below, both after that
    // pruning; identity between the two occurrences is what the canonical
    // table still owes them.
    let answer = expect_ok(interpreter.send(&instance, same, &[]));
    assert!(interpreter.enter(|mutator| matches!(mutator.fetch(&answer), Value::Boolean(true))));
}

/// The table is not a root: a symbol nothing holds does not outlive the
/// collection that finds it unreachable. Compared against a run that mints
/// no symbols at all, so the comparison is not merely two collections in a
/// row finding a heap that has not changed -- which a *strong* table would
/// pass just as well, since nothing would have changed between them either.
#[test]
#[cfg(debug_assertions)]
fn the_symbol_table_does_not_keep_its_objects_alive() {
    let baseline_source = indoc! {"
        Newspeak3
        'T'
        class Baseline = Object new () (
          public make: count = ( ^nil )
        ) : ()
    "};
    let (mut baseline_interpreter, baseline_class) = load(baseline_source);
    let baseline_instance = expect_ok(baseline_interpreter.instantiate(&baseline_class, &[]));
    let baseline_count = baseline_interpreter.stash(Value::SmallInteger(10));
    let baseline_selector = Symbol::intern("make:");
    let _ = expect_ok(baseline_interpreter.send(
        &baseline_instance,
        baseline_selector,
        &[baseline_count],
    ));
    baseline_interpreter.collect_now();
    let baseline_statistics = baseline_interpreter.heap_statistics();

    let transient_source = indoc! {"
        Newspeak3
        'T'
        class Transient = Object new () (
          public make: count = (
            | remaining |
            remaining:: count.
            [remaining > 0] whileTrue: [
              #one.
              #two.
              #three.
              remaining:: remaining - 1.
            ].
            ^nil
          )
        ) : ()
    "};
    let (mut transient_interpreter, transient_class) = load(transient_source);
    let transient_instance = expect_ok(transient_interpreter.instantiate(&transient_class, &[]));
    let transient_count = transient_interpreter.stash(Value::SmallInteger(10));
    let transient_selector = Symbol::intern("make:");
    let _ = expect_ok(transient_interpreter.send(
        &transient_instance,
        transient_selector,
        &[transient_count],
    ));
    transient_interpreter.collect_now();
    let transient_statistics = transient_interpreter.heap_statistics();

    assert_eq!(
        transient_statistics.live_objects, baseline_statistics.live_objects,
        "a symbol nothing holds must not outlive the collection that finds it unreachable"
    );
}
