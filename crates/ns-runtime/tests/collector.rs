//! The collector through a real interpreter: what survives a collection,
//! what does not, and what that does to the object space
//! (`docs/design/memory.md`, "The gates the collector needs").

#![cfg(debug_assertions)]

use indoc::indoc;
use ns_runtime::{Interpreter, Root, Symbol, Value, ValueKind};

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
fn integer_of(value: Value<'_>) -> i64 {
    match value.kind() {
        ValueKind::SmallInteger(integer) => integer,
        _ => panic!("expected a SmallInteger"),
    }
}

/// A module whose entry allocates a great many short-lived closures and
/// keeps none of them. Collecting between runs must give the space back.
#[test]
fn a_collection_gives_back_what_a_run_discarded() {
    // `timesRepeat:` is a kernel-library convenience the bare bootstrap does
    // not load, so the loop is written with `whileTrue:`, which is a native
    // primitive (`crates/ns-runtime/src/builtins/primitive.rs`).
    let source = indoc! {"
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
    let count = interpreter.stash(Value::small_integer(2000));
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
        after_collection.committed_bytes <= after_run.committed_bytes,
        "the buffer must not have grown across a collection"
    );
}

/// §5.1.6: equal symbols are one object. Pruning a symbol nothing holds must
/// not disturb the table's bookkeeping for a different symbol name still
/// being minted: `#kept` is interned twice in one send, after a collection
/// has already pruned `#transient`, and the two occurrences must still be
/// the same object (`docs/design/memory.md`, "Weak references").
#[test]
fn a_symbol_nothing_holds_is_collected_and_minted_again() {
    let source = indoc! {"
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
    assert_eq!(
        interpreter.enter(|mutator| mutator.fetch(&answer).as_boolean()),
        Some(true)
    );
}

/// The table is not a root: a symbol nothing holds does not outlive the
/// collection that finds it unreachable. Compared against a run that mints
/// no symbols at all, so the comparison is not merely two collections in a
/// row finding a heap that has not changed -- which a *strong* table would
/// pass just as well, since nothing would have changed between them either.
#[test]
fn the_symbol_table_does_not_keep_its_objects_alive() {
    let baseline_source = indoc! {"
        class Baseline = Object new () (
          public make: count = ( ^nil )
        ) : ()
    "};
    let (mut baseline_interpreter, baseline_class) = load(baseline_source);
    let baseline_instance = expect_ok(baseline_interpreter.instantiate(&baseline_class, &[]));
    let baseline_count = baseline_interpreter.stash(Value::small_integer(10));
    let baseline_selector = Symbol::intern("make:");
    let _ = expect_ok(baseline_interpreter.send(
        &baseline_instance,
        baseline_selector,
        &[baseline_count],
    ));
    baseline_interpreter.collect_now();
    let baseline_statistics = baseline_interpreter.heap_statistics();

    let transient_source = indoc! {"
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
    let transient_count = transient_interpreter.stash(Value::small_integer(10));
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
fn kept_closures_keep_their_frames() {
    let source = indoc! {"
        class Keeper = Object new (| public blocks ::= nil. |) (
          public fill: count = (
            | made remaining |
            made:: Array new: count.
            remaining:: count.
            [remaining > 0] whileTrue: [
              | captured = remaining. |
              made at: count - remaining + 1 put: [captured + 1].
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
    let count = interpreter.stash(Value::small_integer(50));
    let fill = Symbol::intern("fill:");
    interpreter.collect_now();
    let before = interpreter.heap_statistics();

    let _ = expect_ok(interpreter.send(&instance, fill, &[count]));
    interpreter.collect_now();
    let kept = interpreter.heap_statistics();
    interpreter.collect_now();
    let stable = interpreter.heap_statistics();

    assert_eq!(
        kept.live_objects, stable.live_objects,
        "a second collection must free nothing further"
    );
    // 104: the fifty kept closures, the Array holding them, the loop's
    // condition and body blocks -- `fill:`'s frame keeps its registers,
    // which hold them, and every body-block frame names the body block as
    // its closure -- and 51 frame objects. 51 frames, not 50: each of the fifty kept closures retains
    // the body block's activation for its own iteration of the loop, and
    // `fill:`'s own activation besides. The loop body writes `remaining` and
    // `made`, both outer to it, so `fill:`'s activation was reified as the
    // body block's `home` -- and every kept closure reaches that same one
    // frame through its own body-block frame's `home`, one shared frame
    // beyond the fifty distinct ones. The exact count: a change that shares
    // or drops a frame here changes this number, which a lower-bound check
    // would not catch.
    assert_eq!(
        kept.live_objects - before.live_objects,
        104,
        "fifty kept closures, their Array, the loop's two blocks and 51 frames, {} found",
        kept.live_objects - before.live_objects
    );

    // The loop's first iteration made the closure at index 1, capturing
    // `remaining == 50`. Calling it back after the collection must still
    // answer 51: a frame forwarded to the wrong slot would answer some
    // other, perfectly valid closure's captured value instead of crashing.
    let block_at = Symbol::intern("blockAt:");
    let first_index = interpreter.stash(Value::small_integer(1));
    let first_block = expect_ok(interpreter.send(&instance, block_at, &[first_index]));
    let value_selector = Symbol::intern("value");
    let answer = expect_ok(interpreter.send(&first_block, value_selector, &[]));
    let answer_value = interpreter.enter(|mutator| integer_of(mutator.fetch(&answer)));
    assert_eq!(
        answer_value, 51,
        "the kept closure must still answer with its own captured value after a collection"
    );
}

/// And a run that keeps nothing returns to where it started, which is the
/// whole point of the step: a hundred times the allocation, none of it
/// kept, must leave the same live objects -- frame objects among them --
/// behind as a hundredth as much work would.
#[test]
fn discarding_everything_returns_to_the_starting_size() {
    let source = indoc! {"
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

    let small = interpreter.stash(Value::small_integer(5));
    let _ = expect_ok(interpreter.send(&instance, fill, &[small]));
    interpreter.collect_now();
    let after_small = interpreter.heap_statistics();

    let large = interpreter.stash(Value::small_integer(500));
    let _ = expect_ok(interpreter.send(&instance, fill, &[large]));
    interpreter.collect_now();
    let after_large = interpreter.heap_statistics();

    assert_eq!(
        after_small.live_objects, after_large.live_objects,
        "a hundred times the work, nothing kept, and the same live set"
    );
}

/// The mode that catches premature reclamation: collecting at every
/// safepoint, so a missed root kills its object at the earliest possible
/// moment. The program's answer must not change.
#[test]
fn the_collect_stress_mode_does_not_change_an_answer() {
    let source = indoc! {"
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
    // A real control even under `NS_NEXT_STRESS_COLLECT=1`, which would
    // otherwise turn the "quiet" run stressed too, ambiently, leaving only
    // the absolute `5050` below to catch a difference rather than the
    // quiet-versus-stressed comparison.
    interpreter.set_stress_collect(false);
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let count = interpreter.stash(Value::small_integer(100));
    let selector = Symbol::intern("upTo:");

    let quiet = expect_ok(interpreter.send(&instance, selector, &[count]));
    let quiet_answer = interpreter.enter(|mutator| integer_of(mutator.fetch(&quiet)));
    interpreter.set_stress_collect(true);
    let count_again = interpreter.stash(Value::small_integer(100));
    let stressed = expect_ok(interpreter.send(&instance, selector, &[count_again]));
    let stressed_answer = interpreter.enter(|mutator| integer_of(mutator.fetch(&stressed)));

    assert_eq!(quiet_answer, stressed_answer);
    assert_eq!(quiet_answer, 5050);
    // `moves` cannot show this: an ordinary collection would advance it
    // too. Without this, a schedule whose `due` always answered `false`
    // would still pass everything above.
    assert!(
        interpreter.stress_collections() > 0,
        "the collect stress mode never collected"
    );
}

/// A module whose entry allocates a closure per iteration and keeps none
/// of them: every closure dies young.
const CHURN: &str = indoc! {"
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

/// A module whose entry keeps every closure it makes in an `Array` that
/// lives through the run: the closures survive scavenge after scavenge and
/// are tenured, and the `Array` (40 KB for 5,000 elements) is old from
/// birth.
const KEEPER: &str = indoc! {"
    class Keeper = Object new () (
      public run: count = (
        | kept index |
        kept:: Array new: count.
        index:: 1.
        [index <= count] whileTrue: [
          | captured = index. |
          kept at: index put: [captured].
          index:: index + 1.
        ].
        ^(kept at: count) value
      )
    ) : ()
"};

/// Eden reaching its threshold asks for a scavenge, and the run loop's next
/// safepoint serves it: the closures die young and the run's answer stands.
#[test]
fn filling_eden_scavenges_during_the_run() {
    let (mut interpreter, class) = load(CHURN);
    // The trigger is what this pins; under `NS_NEXT_STRESS_COLLECT=1` a
    // collection at every safepoint empties the nursery before eden ever
    // reaches its threshold, so no scavenge would run.
    interpreter.set_stress_collect(false);
    interpreter.set_scavenge_threshold(16 * 1024);
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let count = interpreter.stash(Value::small_integer(5000));
    let answer = expect_ok(interpreter.send(&instance, Symbol::intern("run:"), &[count]));
    let answer_value = interpreter.enter(|mutator| integer_of(mutator.fetch(&answer)));
    let collector = interpreter.collector_statistics();
    assert_eq!(answer_value, 5000);
    assert!(
        collector.scavenges > 0,
        "eden passed its threshold without a scavenge"
    );
    assert!(interpreter.heap_statistics().young_objects < 5000);
}

/// Nothing asks for a collection until the old space reaches its limit,
/// and then the run loop's next safepoint serves it -- no allocation ever
/// collects (`docs/design/memory.md`, "The trigger and the growth
/// policy"). Closures that die young never reach the old space, so the
/// fixture keeps every one it makes: the scavenges promote what the
/// survivor space cannot hold, the promotions reach the old space's limit,
/// and the full collection runs at the next safepoint.
#[test]
fn allocating_past_the_limit_collects_during_the_run() {
    let (mut interpreter, class) = load(KEEPER);
    // The triggers are what this pins; under `NS_NEXT_STRESS_COLLECT=1`
    // the mode's own collections would count without either trigger
    // firing, and empty the nursery before eden ever reaches its threshold.
    interpreter.set_stress_collect(false);
    // 16 KiB of eden and of old objects: a few hundred closures each, far
    // fewer than the run keeps, so both limits are reached many times over.
    interpreter.set_scavenge_threshold(16 * 1024);
    interpreter.set_collection_floor(16 * 1024);
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let count = interpreter.stash(Value::small_integer(5000));
    let selector = Symbol::intern("run:");

    let answer = expect_ok(interpreter.send(&instance, selector, &[count]));
    let answer_value = interpreter.enter(|mutator| integer_of(mutator.fetch(&answer)));
    let collector = interpreter.collector_statistics();

    assert_eq!(
        answer_value, 5000,
        "a scavenge or a collection during the run must not disturb the program's own answer"
    );
    // The answer alone would also pass an optimization that stops
    // allocating a closure per iteration and so never reaches either limit:
    // the honest witness is that both triggers actually fired.
    assert!(
        collector.scavenges > 0 && collector.collections > 0,
        "the run kept 5000 closures with {} scavenges and {} collections",
        collector.scavenges,
        collector.collections
    );
}

/// Every kind of value a heap object stores -- a `SmallInteger`, a boxed
/// float, a `LargeInteger`, `nil` and another object -- survives being
/// stored as a bare word through a move, a collection and a scavenge at
/// every safepoint: the collector and the scavenger rewrite each word by
/// its offset, and every word reads back as the value that was stored.
#[test]
fn every_kind_of_stored_word_survives_moves_and_collections() {
    let source = indoc! {"
        class Boxes = Object new () (
          class Box integer: anInteger float: aFloat large: aLarge empty: anEmpty next: aNext = Object new (|
            public integer = anInteger.
            public float = aFloat.
            public large = aLarge.
            public empty = anEmpty.
            public next = aNext.
          |) ()
          public build: count = (
            | boxes = Array new: count. previous |
            previous:: Box integer: 0 float: 0.0 large: 0 empty: nil next: nil.
            1 to: count do: [:index |
              previous:: Box
                integer: index
                float: 1.0e300 * index
                large: 18446744073709551616 + index
                empty: nil
                next: previous.
              boxes at: index put: previous].
            ^boxes
          )
          public check: boxes = (
            | sum last |
            sum:: 0.
            boxes do: [:box |
              sum:: sum + box integer + box next integer
                + (box large - 18446744073709551616).
              box float = (1.0e300 * box integer) ifFalse: [^'a float changed'].
              box empty isNil ifFalse: [^'a nil field changed']].
            last:: boxes at: boxes size.
            ^sum printString, ' ', (last float isKindOfFloat ifTrue: ['Float'] ifFalse: ['no Float']), ' ', (last float = 1.0e303) printString,
              ' ', last large printString,
              ' ', last empty printString, ' ', last next integer printString
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    interpreter.set_stress_moves(true);
    interpreter.set_stress_collect(true);
    interpreter.set_stress_scavenge(true);
    let instance = expect_ok(interpreter.instantiate(&class, &[]));
    let count = interpreter.stash(Value::small_integer(1000));
    let boxes = expect_ok(interpreter.send(&instance, Symbol::intern("build:"), &[count]));
    let answer = expect_ok(interpreter.send(&instance, Symbol::intern("check:"), &[boxes]));
    let text = interpreter.enter(|mutator| {
        let Some(string) = mutator.fetch(&answer).as_object() else {
            panic!("check: answers a String")
        };
        mutator.heap().str_value(string).expect("a String")
    });

    // Each box adds its index twice and its predecessor's once:
    // 2 * 500500 + 499500.
    assert_eq!(
        text, "1500500 Float true 18446744073709552616 nil 999",
        "every stored word reads back as the value stored"
    );
    assert!(interpreter.moves() > 0, "the stress modes never moved");
    assert!(
        interpreter.stress_collections() > 0,
        "the collect stress mode never collected"
    );
    assert!(
        interpreter.stress_scavenges() > 0,
        "the scavenge stress mode never scavenged"
    );
}
