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
