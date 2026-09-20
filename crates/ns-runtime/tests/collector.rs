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

/// The `SmallInteger` a `Value` holds, or a panic. Used by a test that calls
/// a kept closure back after a collection and checks what it answers, not
/// merely that it still exists.
fn integer_of(value: &Value<'_>) -> i64 {
    match value {
        Value::SmallInteger(integer) => *integer,
        _ => panic!("expected a SmallInteger"),
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

/// What a program keeps, the collector keeps. A closure held in a slot
/// retains its captured frame, so fifty kept closures leave at least fifty
/// frames behind -- the retention shape closure conversion left in place,
/// and the one `bench_block.ns` measures. Existence alone would not catch a
/// frame forwarded to the wrong slot: that mistake still answers something,
/// just a different, perfectly valid frame's captured value, so the test
/// also calls one of the kept closures back after the collection and checks
/// the number it answers, not merely that it answers at all.
#[test]
#[cfg(debug_assertions)]
fn kept_closures_keep_their_frames() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Keeper = Object new (| public blocks ::= nil. |) (
          public fill: count = (
            | made remaining |
            made:: OrderedCollection new.
            remaining:: count.
            [remaining > 0] whileTrue: [
              | captured = remaining. |
              made add: [captured + 1].
              remaining:: remaining - 1.
            ].
            blocks:: made.
            ^nil
          )
          public blockAt: index = (
            ^blocks at: index
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let count = interpreter.stash(Value::SmallInteger(50));
    let fill = Symbol::intern("fill:");

    let _ = expect_ok(interpreter.send(&instance, fill, &[count]));
    interpreter.collect_now();
    let kept = interpreter.heap_statistics();
    interpreter.collect_now();
    let stable = interpreter.heap_statistics();

    assert_eq!(
        kept.live_objects, stable.live_objects,
        "a second collection must free nothing further"
    );
    assert_eq!(
        kept.live_frames, stable.live_frames,
        "a second collection must not disturb the frames the kept closures hold"
    );
    assert!(
        kept.live_frames >= 50,
        "each kept closure retains the frame it captured, {} found",
        kept.live_frames
    );

    // The loop's first iteration made the closure at index 1, capturing
    // `remaining == 50`. Calling it back after the collection must still
    // answer 51: a frame forwarded to the wrong slot would answer some
    // other, perfectly valid closure's captured value instead of crashing.
    let block_at = Symbol::intern("blockAt:");
    let first_index = interpreter.stash(Value::SmallInteger(1));
    let first_block = expect_ok(interpreter.send(&instance, block_at, &[first_index]));
    let value_selector = Symbol::intern("value");
    let answer = expect_ok(interpreter.send(&first_block, value_selector, &[]));
    let answer_value = interpreter.enter(|mutator| integer_of(&mutator.fetch(&answer)));
    assert_eq!(
        answer_value, 51,
        "the kept closure must still answer with its own captured value after a collection"
    );
}

/// And a run that keeps nothing returns to where it started, which is the
/// whole point of the step: a hundred times the allocation, none of it
/// kept, must leave the same live objects and the same live frames behind
/// as a hundredth as much work would.
#[test]
#[cfg(debug_assertions)]
fn discarding_everything_returns_to_the_starting_size() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Discard = Object new () (
          public fill: count = (
            | remaining |
            remaining:: count.
            [remaining > 0] whileTrue: [
              | captured = remaining. |
              [captured + 1] value.
              remaining:: remaining - 1.
            ].
            ^nil
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let fill = Symbol::intern("fill:");

    let small = interpreter.stash(Value::SmallInteger(5));
    let _ = expect_ok(interpreter.send(&instance, fill, &[small]));
    interpreter.collect_now();
    let after_small = interpreter.heap_statistics();

    let large = interpreter.stash(Value::SmallInteger(500));
    let _ = expect_ok(interpreter.send(&instance, fill, &[large]));
    interpreter.collect_now();
    let after_large = interpreter.heap_statistics();

    assert_eq!(
        after_small.live_objects, after_large.live_objects,
        "a hundred times the work, nothing kept, and the same live set"
    );
    assert_eq!(
        after_small.live_frames, after_large.live_frames,
        "a hundred times the work, nothing kept, and the same live frames"
    );
}

/// The mode that catches premature reclamation: collecting at every
/// safepoint, so a missed root kills its object at the earliest possible
/// moment. The program's answer must not change.
#[test]
#[cfg(debug_assertions)]
fn the_collect_stress_mode_does_not_change_an_answer() {
    let source = indoc! {"
        Newspeak3
        'T'
        class Sum = Object new () (
          public upTo: count = (
            | total remaining |
            total:: 0.
            remaining:: count.
            [remaining > 0] whileTrue: [
              total:: total + remaining.
              remaining:: remaining - 1.
            ].
            ^total
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let count = interpreter.stash(Value::SmallInteger(100));
    let selector = Symbol::intern("upTo:");

    let quiet = expect_ok(interpreter.send(&instance, selector, &[count]));
    let quiet_answer = interpreter.enter(|mutator| integer_of(&mutator.fetch(&quiet)));
    interpreter.set_stress_collect(true);
    let count_again = interpreter.stash(Value::SmallInteger(100));
    let stressed = expect_ok(interpreter.send(&instance, selector, &[count_again]));
    let stressed_answer = interpreter.enter(|mutator| integer_of(&mutator.fetch(&stressed)));

    assert_eq!(quiet_answer, stressed_answer);
    assert_eq!(quiet_answer, 5050);
}

/// Nothing asks for a collection until an arena reaches its limit, and
/// then the run loop's next safepoint serves it -- no allocation ever
/// collects (`docs/design/memory.md`, "The trigger and the growth
/// policy").
#[test]
#[cfg(debug_assertions)]
fn allocating_past_the_limit_collects_during_the_run() {
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
    interpreter.set_collection_floor(256);
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let count = interpreter.stash(Value::SmallInteger(5000));
    let selector = Symbol::intern("run:");

    let _ = expect_ok(interpreter.send(&instance, selector, &[count]));
    let after = interpreter.heap_statistics();

    assert!(
        after.live_objects < 5000,
        "the run collected nothing: {} live objects after 5000 closures",
        after.live_objects
    );
}
