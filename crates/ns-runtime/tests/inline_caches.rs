//! The inline caches (`docs/design/dispatch.md`, "3b — The caches"): what a
//! site records for the receivers it sees. Every program here also answers
//! what the uncached path answers; the goldens pin that under
//! both lowerings, these pin the caches' own states.

#[cfg(debug_assertions)]
use indoc::formatdoc;
use indoc::indoc;
use ns_runtime::{
    CachedTarget, CachedTargetKind, CallSiteState, HostResult, HostSignal, Interpreter, Lowering,
    MethodSide, Root, SourceOrigin, Symbol, Value, ValueKind,
};

fn load_with(source: &str, lowering: Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(module, source.into(), SourceOrigin::File("test.ns".into()))
        .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    // Every test in this file pins what a site or the global cache holds
    // right after a run; the cache stress mode vacates that at every
    // safepoint by design (`docs/design/dispatch.md`, "Invalidation"), so it
    // is turned off here regardless of `NS_NEXT_STRESS_CACHES`, leaving
    // `tests/stress.rs`'s own cache-stress test -- which checks only the
    // answer, never the cache state -- as the one that actually runs under
    // it.
    #[cfg(debug_assertions)]
    interpreter.set_stress_caches(false);
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
}

fn load(source: &str) -> (Interpreter, Root) {
    load_with(source, Lowering::Optimized)
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

/// Sends `selector` to `receiver` with `arguments`, expecting a value.
fn send(
    interpreter: &mut Interpreter,
    receiver: &Root,
    selector: &str,
    arguments: &[Root],
) -> Root {
    expect_ok(
        interpreter.send(receiver, Symbol::intern(selector), arguments),
        selector,
    )
}

fn instantiate(interpreter: &mut Interpreter, class: &Root) -> Root {
    expect_ok(interpreter.instantiate(class, &[]), "instantiate")
}

fn small_integer(interpreter: &mut Interpreter, root: &Root) -> i64 {
    interpreter.enter(|mutator| match mutator.fetch(root).kind() {
        ValueKind::SmallInteger(integer) => integer,
        _ => panic!("expected a small integer"),
    })
}

fn integer_argument(interpreter: &Interpreter, integer: i64) -> Root {
    interpreter.stash(Value::small_integer(integer))
}

fn states(
    interpreter: &mut Interpreter,
    class: &Root,
    side: MethodSide,
    method: &str,
    sent: &str,
) -> Vec<CallSiteState> {
    interpreter.call_site_states(class, side, Symbol::intern(method), Symbol::intern(sent))
}

fn public(kind: CachedTargetKind) -> CachedTarget {
    CachedTarget {
        kind,
        private: false,
    }
}

fn method(selector: &str) -> CachedTarget {
    public(CachedTargetKind::Method(Symbol::intern(selector)))
}

/// Two `Inner` instances from two `Outer` instances share one monomorphic
/// entry: the cache guards on the application, not on the class object.
#[test]
fn nested_class_instances_of_two_enclosing_objects_share_one_entry() {
    let source = indoc! {"
        class Subject = Object new () (
          class Outer = Object new () (
            public class Inner = Object new () ( public describe = ( ^1 ) )
          )
          public run = (
            | sum |
            sum:: 0.
            {Outer new Inner new. Outer new Inner new. Outer new Inner new}
              do: [:each | sum:: sum + each describe].
            ^sum
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 3);
    assert_eq!(
        states(
            &mut interpreter,
            &class,
            MethodSide::Instance,
            "run",
            "describe"
        ),
        vec![CallSiteState::Monomorphic(method("describe"))]
    );
}

fn nested_class(name: &str, field: Option<u32>, private: bool) -> CachedTarget {
    CachedTarget {
        kind: CachedTargetKind::NestedClass {
            name: Symbol::intern(name),
            field,
        },
        private,
    }
}

/// A nested-class accessor's entry records the absolute index of the
/// receiver's hidden slot -- after `Holder`'s two slots, `Object` having
/// none, so 2 -- and the second send, a hit, answers the class the first
/// created.
#[test]
fn a_nested_class_hit_reads_the_cached_field() {
    let source = indoc! {"
        class Subject = Object new () (
          class Holder = Object new (
            | first ::= 1. second ::= 2. |
          ) (
            public class Inner = Object new () ()
          )
          public run = (
            | holder classes |
            holder:: Holder new.
            classes:: Array new: 2.
            1 to: 2 do: [:index | classes at: index put: holder Inner].
            ^(classes at: 1) == (classes at: 2) ifTrue: [1] ifFalse: [0]
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 1);
    assert_eq!(
        states(
            &mut interpreter,
            &class,
            MethodSide::Instance,
            "run",
            "Inner"
        ),
        vec![CallSiteState::Monomorphic(nested_class(
            "Inner",
            Some(2),
            false
        ))]
    );
}

/// A private site's guard is N's application, not the receiver's, so its
/// entry records the hidden slot's index in N's application, and a hit
/// uses it only for a receiver of exactly that application: a subclass's
/// instance takes the lookup. Each enclosing object still answers a class
/// of its own, the same on every send.
#[test]
fn a_private_nested_class_hit_reads_the_field_only_for_the_guard_s_application() {
    let source = indoc! {"
        class Subject = Object new () (
          class Base = Object new (
            | first ::= 1. |
          ) (
            private class Inner = Object new () ()
            public inner = ( ^Inner )
          )
          class Derived = Base new (
            | second ::= 2. |
          ) ()
          public baseClass = ( ^Base )
          public run = (
            | derived base answers |
            derived:: Derived new.
            base:: Base new.
            answers:: {derived inner. base inner. derived inner. base inner}.
            (answers at: 1) == (answers at: 3) ifFalse: [^1].
            (answers at: 2) == (answers at: 4) ifFalse: [^2].
            (answers at: 1) == (answers at: 2) ifTrue: [^3].
            ^0
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 0);
    let base = send(&mut interpreter, &instance, "baseClass", &[]);
    assert_eq!(
        states(
            &mut interpreter,
            &base,
            MethodSide::Instance,
            "inner",
            "Inner"
        ),
        vec![CallSiteState::Monomorphic(nested_class(
            "Inner",
            Some(1),
            true
        ))]
    );
}

/// A `super` site's guard is S's application, and a hidden slot's index
/// is the same in every application over the one that declares it, so its
/// entry records the index in S's layout -- `Base`'s one slot, then the
/// hidden slot, so 1 -- and a hit reads it for a receiver of the running
/// class's application. A subclass's instance takes the lookup; each
/// enclosing object still answers a class of its own, the same on every
/// send. A body-less declaration's own `super Inner` starts at its class
/// object's superclass, `Base`, as every `super` starts at its current
/// class's, and finds `Base`'s.
#[test]
fn a_super_nested_class_hit_reads_the_field_in_s_layout() {
    let source = indoc! {"
        class Subject = Object new () (
          class Base = Object new (
            | first ::= 1. |
          ) (
            public class Inner = Object new () ()
          )
          class Holder = Base new (
            | second ::= 2. |
          ) (
            public baseInner = ( ^super Inner )
          )
          class Deeper = Holder new (
            | third ::= 3. |
          ) ()
          class Marker tag: t = Object new (| public a = t. |) ()
          class Applied = Base new <: Marker tag: super Inner.
          public holderClass = ( ^Holder )
          public run = (
            | holder deeper answers applied |
            holder:: Holder new.
            deeper:: Deeper new.
            answers:: {holder baseInner. holder baseInner. deeper baseInner. deeper baseInner}.
            (answers at: 1) == (answers at: 2) ifFalse: [^1].
            (answers at: 3) == (answers at: 4) ifFalse: [^2].
            (answers at: 1) == (answers at: 3) ifTrue: [^3].
            applied:: Applied new.
            applied a name = 'Inner' ifFalse: [^4].
            ^0
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 0);
    let holder = send(&mut interpreter, &instance, "holderClass", &[]);
    assert_eq!(
        states(
            &mut interpreter,
            &holder,
            MethodSide::Instance,
            "baseInner",
            "Inner"
        ),
        vec![CallSiteState::Monomorphic(nested_class(
            "Inner",
            Some(1),
            false
        ))]
    );
}

const FIVE_KINDS: &str = indoc! {"
    class Subject = Object new () (
      class KindOne = Object new () ( public value = ( ^1 ) )
      class KindTwo = Object new () ( public value = ( ^2 ) )
      class KindThree = Object new () ( public value = ( ^3 ) )
      class KindFour = Object new () ( public value = ( ^4 ) )
      class KindFive = Object new () ( public value = ( ^5 ) )
      public run: count = (
        | sum index all |
        sum:: 0.
        index:: 0.
        all:: {KindOne new. KindTwo new. KindThree new. KindFour new. KindFive new}.
        [index < count] whileTrue: [
          index:: index + 1.
          sum:: sum + (all at: index) value].
        ^sum
      )
    ) : ()
"};

/// A site that sees a second type goes polymorphic, keeps four in the order
/// first seen, and a fifth makes it megamorphic.
#[test]
fn a_site_goes_polymorphic_then_megamorphic() {
    let (mut interpreter, class) = load(FIVE_KINDS);
    let instance = instantiate(&mut interpreter, &class);
    let value_states = |interpreter: &mut Interpreter| {
        states(interpreter, &class, MethodSide::Instance, "run:", "value")
    };
    let two = integer_argument(&interpreter, 2);
    let answer = send(&mut interpreter, &instance, "run:", &[two]);
    assert_eq!(small_integer(&mut interpreter, &answer), 3);
    assert_eq!(
        value_states(&mut interpreter),
        vec![CallSiteState::Polymorphic(vec![
            method("value"),
            method("value")
        ])]
    );
    let four = integer_argument(&interpreter, 4);
    send(&mut interpreter, &instance, "run:", &[four]);
    assert_eq!(
        value_states(&mut interpreter),
        vec![CallSiteState::Polymorphic(vec![method("value"); 4])]
    );
    let five = integer_argument(&interpreter, 5);
    let answer = send(&mut interpreter, &instance, "run:", &[five]);
    assert_eq!(small_integer(&mut interpreter, &answer), 15);
    assert_eq!(
        value_states(&mut interpreter),
        vec![CallSiteState::Megamorphic]
    );
}

/// `true` and `false` are instances of two classes: a site that is not
/// inlined and sees both is bimorphic. `not` is a kernel primitive method
/// (`Kernel.ns`), so each entry's target is a
/// `CachedTargetKind::Method`, not a `CachedTargetKind::Primitive`.
#[test]
fn a_site_on_true_and_false_is_bimorphic() {
    let source = indoc! {"
        class Subject = Object new () (
          public run = (
            | count |
            count:: 0.
            {true. false. true} do: [:each | each not ifTrue: [count:: count + 1]].
            ^count
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 1);
    let not = method("not");
    assert_eq!(
        states(&mut interpreter, &class, MethodSide::Instance, "run", "not"),
        vec![CallSiteState::Polymorphic(vec![not.clone(), not])]
    );
}

/// `run` keeps its two cells in `cells`, so that both applications of
/// `Cell` stay live after it: a full collection between the run and the
/// reading of a site -- the collection stress mode makes one at every
/// safepoint -- frees the guard of an application nothing holds, and
/// vacates every entry naming it.
const TWO_APPLICATIONS: &str = indoc! {"
    class Subject = Object new (| cells |) (
      class Narrow = Object new (| first ::= 1. |) ( public describe = ( ^1 ) )
      class Wide = Object new (| first ::= 1. second ::= 2. third ::= 3. |) ( public describe = ( ^2 ) )
      class Box on: base = Object new (| base = base. |) (
        public class Cell = base new (| public held ::= 10. |) (
          public describe = ( ^super describe + 10 )
        )
      )
      public narrowCell = ( ^(Box on: Narrow) Cell )
      public run = (
        | sum |
        sum:: 0.
        cells:: {(Box on: Narrow) Cell new. (Box on: Wide) Cell new}.
        cells do: [:each | sum:: sum + each held + each describe].
        ^sum
      )
    ) : ()
"};

/// One slot accessor under two applications of one mixin with different
/// inherited slots: two entries, each with its own absolute field -- `held`
/// after one inherited slot, and after three.
#[test]
fn a_slot_accessor_under_two_applications_has_two_fields() {
    let (mut interpreter, class) = load(TWO_APPLICATIONS);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 43);
    assert_eq!(
        states(
            &mut interpreter,
            &class,
            MethodSide::Instance,
            "run",
            "held"
        ),
        vec![CallSiteState::Polymorphic(vec![
            public(CachedTargetKind::Slot { field: 1 }),
            public(CachedTargetKind::Slot { field: 3 }),
        ])]
    );
}

/// `super` in a mixin applied under two superclasses: the key is the running
/// method's application, so the one `super describe` site holds one entry per
/// application of `Cell`, each naming its own superclass's method.
#[test]
fn a_super_send_in_a_mixin_applied_twice() {
    let (mut interpreter, class) = load(TWO_APPLICATIONS);
    let instance = instantiate(&mut interpreter, &class);
    send(&mut interpreter, &instance, "run", &[]);
    let cell = send(&mut interpreter, &instance, "narrowCell", &[]);
    assert_eq!(
        states(
            &mut interpreter,
            &cell,
            MethodSide::Instance,
            "describe",
            "describe"
        ),
        vec![CallSiteState::Polymorphic(vec![
            method("describe"),
            method("describe")
        ])]
    );
}

/// A private lexical site whose `o` changes object and class stays
/// monomorphic: its guard is N's application, which does not change, and
/// §5.9 runs N's private method on every `o`.
#[test]
fn a_private_lexical_site_with_a_changing_receiver_stays_monomorphic() {
    let source = indoc! {"
        class Subject = Object new () (
          class Outer named: label = Object new (| name = label. |) (
            private secret = ( ^name )
            public class Inner = Object new () (
              public reveal = ( ^secret )
            )
          )
          class Sub named: label = Outer named: label () (
            public secret = ( ^0 )
          )
          public anInner = ( ^(Outer named: 1) Inner )
          public run = (
            | sum |
            sum:: 0.
            {Outer named: 1. Sub named: 2. Outer named: 3. Sub named: 4}
              do: [:each | sum:: sum + each Inner new reveal].
            ^sum
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 10);
    let inner = send(&mut interpreter, &instance, "anInner", &[]);
    assert_eq!(
        states(
            &mut interpreter,
            &inner,
            MethodSide::Instance,
            "reveal",
            "secret"
        ),
        vec![CallSiteState::Monomorphic(CachedTarget {
            kind: CachedTargetKind::Method(Symbol::intern("secret")),
            private: true,
        })]
    );
}

/// Class objects and instances at one site: a factory, a class-side method
/// and an instance method, three entries.
#[test]
fn class_objects_and_instances_share_a_site() {
    let source = indoc! {"
        class Subject = Object new () (
          class Maker make = Object new () ( public describe = ( ^1 ) )
          class Plain = Object new () ( public describe = ( ^2 ) ) : ( public make = ( ^Plain new ) )
          class Custom = Object new () ( public describe = ( ^3 ) public make = ( ^self ) )
          public run = (
            | sum |
            sum:: 0.
            {Maker. Plain. Custom new} do: [:each | sum:: sum + each make describe].
            ^sum
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 6);
    assert_eq!(
        states(
            &mut interpreter,
            &class,
            MethodSide::Instance,
            "run",
            "make"
        ),
        vec![CallSiteState::Polymorphic(vec![
            public(CachedTargetKind::Factory),
            method("make"),
            method("make"),
        ])]
    );
}

/// `doesNotUnderstand:` with a user handler and with a slot setter named
/// `doesNotUnderstand` (§6.3.2) at one site: two entries.
#[test]
fn does_not_understand_handlers_are_entries_like_any_other() {
    let source = indoc! {"
        class Subject = Object new () (
          class Catcher = Object new () ( public doesNotUnderstand: message = ( ^1 ) )
          class Recorder = Object new (| public doesNotUnderstand ::= nil. |) ()
          public run = (
            | sum |
            sum:: 0.
            {Catcher new. Recorder new. Catcher new} do: [:each | | answer |
              answer:: each zork.
              sum:: sum + ((answer == each) ifTrue: [10] ifFalse: [answer])].
            ^sum
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 12);
    let handler = public(CachedTargetKind::DoesNotUnderstand);
    assert_eq!(
        states(
            &mut interpreter,
            &class,
            MethodSide::Instance,
            "run",
            "zork"
        ),
        vec![CallSiteState::Polymorphic(vec![handler.clone(), handler])]
    );
}

/// A second call site with the same guard, selector and rule as a first --
/// two different methods sending `value` to the same receiver kind -- fills
/// straight from the global cache the first send left behind, and leaves the
/// same state a direct resolve would: `Monomorphic`.
#[test]
fn a_second_site_fills_from_the_global_cache() {
    let source = indoc! {"
        class Subject = Object new () (
          class Kind = Object new () ( public value = ( ^1 ) )
          public first = ( ^Kind new value )
          public second = ( ^Kind new value )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let first = send(&mut interpreter, &instance, "first", &[]);
    assert_eq!(small_integer(&mut interpreter, &first), 1);
    let second = send(&mut interpreter, &instance, "second", &[]);
    assert_eq!(small_integer(&mut interpreter, &second), 1);
    assert_eq!(
        states(
            &mut interpreter,
            &class,
            MethodSide::Instance,
            "second",
            "value"
        ),
        vec![CallSiteState::Monomorphic(method("value"))]
    );
}

/// A `Lexical` entry's key covers N's application too
/// (`docs/design/dispatch.md`, "3b -- The caches"), so it is right for any
/// site under the same N: two distinct lexical sites in different methods of
/// the same class, both sending the same implicit-receiver selector to an
/// enclosing object of the same application, share one global entry rather
/// than each filling its own. This does not by itself show the second site
/// *read* that entry instead of resolving again and overwriting it -- both
/// leave one entry behind, and `filled_global_entries` cannot tell them
/// apart; `a_vacant_lexical_site_does_not_borrow_a_different_ns_entry` below
/// is what actually depends on N being part of the `Lexical` rule's payload,
/// since it goes red the moment that payload is dropped from the comparison.
#[test]
fn a_vacant_lexical_site_uses_the_global_cache() {
    let source = indoc! {"
        class Subject = Object new () (
          public helper = ( ^1 )
          public first = ( ^helper )
          public second = ( ^helper )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    // The bootstrap runs shipped code of its own (the platform's boot), so
    // the count is taken from what it left.
    let before = interpreter.cache_statistics().filled_global_entries;
    let first_instance = instantiate(&mut interpreter, &class);
    let second_instance = instantiate(&mut interpreter, &class);
    send(&mut interpreter, &first_instance, "first", &[]);
    send(&mut interpreter, &second_instance, "second", &[]);
    let statistics = interpreter.cache_statistics();
    assert_eq!(
        statistics.filled_global_entries - before,
        1,
        "both sites must share one global entry"
    );
}

/// The behavioural pin N's place in the `Lexical` rule's payload exists for.
/// `Derived` overrides `Base`'s private `helper` with a public one;
/// `fromDerived` (N = `Derived`) fills the `Lexical` row under `(Derived`'s
/// dispatch id, `#helper`, `Lexical(Derived`'s application))`. `fromBase`'s
/// own site is then still vacant, and reaches the very same `o` -- the same
/// `Derived` instance, so the same dispatch id -- under the same selector,
/// but with N = `Base`, which declares `helper` private. Without N in the
/// key, that still-vacant site would read `Derived`'s entry and answer with
/// `Derived`'s public `helper` instead of running its own N's private one:
/// §5.9's private rule silently skipped, wrong output, exit status 0. With
/// N in the key the read misses (different `Lexical` payload), `fromBase`
/// resolves independently and finds `Base`'s private `helper`, and the
/// answer is `21`, not `22`.
///
/// I verified this test actually depends on N's payload by temporarily
/// neutralizing it -- `GlobalCache::group` hashing every `Lexical(_)` as
/// `Lexical(DispatchId::NONE)` and `GlobalEntry::holds` comparing rules the
/// same stripped way, so two different N's hash to the same group and
/// compare equal -- and confirming this test fails (`assertion failed: left
/// == 22, right == 21`) while the rest of the workspace suite, including
/// `a_vacant_lexical_site_uses_the_global_cache` above, stays green; then
/// restored the real implementation and confirmed this test passes again.
#[test]
fn a_vacant_lexical_site_does_not_borrow_a_different_ns_entry() {
    let source = indoc! {"
        class Subject = Object new () (
          class Base = Object new () (
            private helper = ( ^1 )
            public fromBase = ( ^helper )
          )
          class Derived = Base new () (
            public helper = ( ^2 )
            public fromDerived = ( ^helper )
          )
          public baseClass = ( ^Base )
          public run = (
            | derived |
            derived:: Derived new.
            ^(derived fromDerived) * 10 + (derived fromBase)
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    assert_eq!(
        small_integer(&mut interpreter, &answer),
        21,
        "fromBase must run Base's own private helper, not Derived's public override"
    );
    let base_class = send(&mut interpreter, &instance, "baseClass", &[]);
    assert_eq!(
        states(
            &mut interpreter,
            &base_class,
            MethodSide::Instance,
            "fromBase",
            "helper"
        ),
        vec![CallSiteState::Monomorphic(CachedTarget {
            kind: CachedTargetKind::Method(Symbol::intern("helper")),
            private: true,
        })],
        "fromBase's site must have decided itself a private site"
    );
}

/// An invalidation vacates every site, every polymorphic block and the
/// global cache, and keeps every family's range, since running activations
/// keep their bases; the next run fills the sites again and answers the same
/// (`docs/design/dispatch.md`, "Invalidation").
#[test]
fn an_invalidation_vacates_every_site_and_keeps_the_ranges() {
    let (mut interpreter, class) = load(FIVE_KINDS);
    let instance = instantiate(&mut interpreter, &class);
    let five = integer_argument(&interpreter, 5);
    send(&mut interpreter, &instance, "run:", &[five]);
    let before = interpreter.cache_statistics();
    assert_eq!(
        before.megamorphic_sites, 1,
        "only the `value` site saw five kinds"
    );
    assert!(before.filled_global_entries > 0);
    interpreter.invalidate_caches();
    let after = interpreter.cache_statistics();
    assert_eq!(after.site_entries, before.site_entries, "the ranges stay");
    assert_eq!(after.filled_sites, 0);
    assert_eq!(after.megamorphic_sites, 0);
    assert_eq!(after.polymorphic_blocks_in_use, 0);
    assert_eq!(after.filled_global_entries, 0);
    assert_eq!(
        states(
            &mut interpreter,
            &class,
            MethodSide::Instance,
            "run:",
            "value"
        ),
        vec![CallSiteState::Vacant]
    );
    let one = integer_argument(&interpreter, 1);
    let answer = send(&mut interpreter, &instance, "run:", &[one]);
    assert_eq!(small_integer(&mut interpreter, &answer), 1);
    assert_eq!(
        states(
            &mut interpreter,
            &class,
            MethodSide::Instance,
            "run:",
            "value"
        ),
        vec![CallSiteState::Monomorphic(method("value"))]
    );
}

/// A global cache of one or two groups, where every insert contends for the
/// same two-slot group, answers what a full one answers: the megamorphic
/// `value` site resolves through it on every send.
#[test]
fn a_tiny_global_cache_answers_like_a_full_one() {
    for entries in [2, 4] {
        let (mut interpreter, class) = load(FIVE_KINDS);
        interpreter.set_global_cache_size(entries);
        let instance = instantiate(&mut interpreter, &class);
        for _ in 0..3 {
            let five = integer_argument(&interpreter, 5);
            let answer = send(&mut interpreter, &instance, "run:", &[five]);
            assert_eq!(small_integer(&mut interpreter, &answer), 15);
        }
        assert_eq!(interpreter.cache_statistics().global_entries, entries);
    }
}

/// Only the families that ran take room in the table: `used`'s own two sends
/// (`+` in its block, `do:`) and the family of the `Array>>do:` it ran --
/// its loop's sends and its failure block's `super do:`, ten entries,
/// twelve in all -- and nothing for `unused` or for an initializer with no
/// sends.
#[test]
fn only_the_families_that_ran_take_room_in_the_cache_table() {
    let source = indoc! {"
        class Subject = Object new () (
          public used = (
            | sum |
            sum:: 0.
            {1. 2} do: [:each | sum:: sum + each].
            ^sum
          )
          public unused = ( ^1 + 2 + 3 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    // The bootstrap runs shipped code of its own (the platform's boot), so
    // the table is measured from what it left.
    let booted = interpreter.cache_statistics().site_entries;
    let instance = instantiate(&mut interpreter, &class);
    assert_eq!(interpreter.cache_statistics().site_entries, booted);
    let answer = send(&mut interpreter, &instance, "used", &[]);
    assert_eq!(small_integer(&mut interpreter, &answer), 3);
    assert_eq!(interpreter.cache_statistics().site_entries, booted + 12);
}

/// A cached kernel method whose leaf declines on a receiver of its own class
/// keeps its entry: `Value>>=` answers from its leaf for a `Pair` of
/// integers, and declines for a `Pair` holding a `Loud`, whose class brings
/// its own `=` a leaf may not send -- the method is then activated and its
/// failure block, `valueEquals:`, sends it, which is why the second answer is
/// `Loud`'s `false` rather than the `true` two equal states would give. Both
/// receivers share one application, so the site stays monomorphic on the one
/// method entry.
#[test]
fn a_failing_primitive_keeps_its_entry() {
    let source = indoc! {"
        class Subject = Object <: Value () (
          class Pair of: value = Value new (
            | public value = value. |
          ) ()
          class Loud = Value new () (
            public = other = ( ^false )
            public hash = ( ^0 )
          )
          public run = (
            | answers index |
            answers:: Array new: 2. index:: 0.
            {Pair of: 1. Pair of: Loud new} do: [:each | index:: index + 1. answers at: index put: each = each].
            ^(answers at: 1) & (answers at: 2) not
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let instance = instantiate(&mut interpreter, &class);
    let answer = send(&mut interpreter, &instance, "run", &[]);
    interpreter.enter(|mutator| {
        assert_eq!(mutator.fetch(&answer).as_boolean(), Some(true));
    });
    assert_eq!(
        states(&mut interpreter, &class, MethodSide::Instance, "run", "="),
        vec![CallSiteState::Monomorphic(method("="))]
    );
}

const TWO_FACTORIES: &str = indoc! {"
    class Subject = Object new () (
      class Alpha = Object new () ( public describe = ( ^1 ) )
      class Beta with: seed = Object new () ( public describe = ( ^2 ) )
      public makeAll: order = (
        | sum |
        sum:: 0.
        order do: [:each | sum:: sum * 10 + each new describe].
        ^sum
      )
      public alphaThenBeta = ( ^self makeAll: {Alpha. Beta} )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// A `Factory` entry records the chain and nothing else, because
    /// `newspeak-spec.pdf` §6.3 instantiates the *receiver*; a hit reads the
    /// class out of the send's key rather than out of the entry.
    ///
    /// The case that says so is **one guard, two receivers**, which two
    /// declarations could never give: a nested class has one class object per
    /// enclosing instance (§6) and they all share one application, so the second
    /// and third sends here are monomorphic *hits* on the entry the first filled.
    /// Each `Item`'s `describe` reads its own enclosing `Holder`'s slot, so the
    /// answer is 121 when each hit instantiates the class object it was sent to
    /// and 111 if a hit ever handed back the class object the entry was recorded
    /// for. The site stays monomorphic, which is what proves the two later sends
    /// were hits and not misses that re-ran the lookup.
    fn a_cached_factory_instantiates_the_receiver_it_is_sent_to(lowering) {
        let source = indoc! {"
            class Subject = Object new () (
              class Holder seed: value = Object new (| seed = value. |) (
                class Item = Object new () ( public describe = ( ^seed ) )
                public item = ( ^Item )
              )
              public run = (
                | first second sum |
                first:: Holder seed: 1.
                second:: Holder seed: 2.
                sum:: 0.
                {first item. second item. first item}
                  do: [:each | sum:: sum * 10 + each new describe].
                ^sum
              )
            ) : ()
        "};
        let (mut interpreter, class) = load_with(source, lowering);
        let instance = instantiate(&mut interpreter, &class);
        let answer = send(&mut interpreter, &instance, "run", &[]);
        assert_eq!(
            small_integer(&mut interpreter, &answer),
            121,
            "{lowering:?}"
        );
        if lowering == Lowering::Optimized {
            assert_eq!(
                states(&mut interpreter, &class, MethodSide::Instance, "run", "new"),
                vec![CallSiteState::Monomorphic(public(
                    CachedTargetKind::Factory
                ))]
            );
        }
    }
}

ns_runtime::for_each_lowering! {
    /// The guard is the receiver's metaclass's instances id -- a class object's
    /// dispatch id is its metaclass's, since its class is its metaclass -- so a
    /// `Factory` entry recorded for a class whose factory selector is `new`
    /// cannot answer for `Beta`, whose factory is `with:`. §3.2's flat metaclass
    /// is what makes the entry safe without a re-check: `new` is not a member of
    /// `Beta`'s metaclass, and no class side is inherited for it to come from.
    fn a_cached_factory_does_not_answer_a_class_with_another_factory_selector(lowering) {
        let (mut interpreter, class) = load_with(TWO_FACTORIES, lowering);
        let instance = instantiate(&mut interpreter, &class);
        match interpreter.send(&instance, Symbol::intern("alphaThenBeta"), &[]) {
            Err(HostSignal::Error(error)) => assert_eq!(error.selector, Symbol::NEW),
            Ok(value) => {
                let got =
                    interpreter.enter(|mutator| mutator.heap().describe(mutator.fetch(&value)));
                panic!("expected doesNotUnderstand: new, got Ok({got})")
            }
            Err(HostSignal::Return) => panic!("expected doesNotUnderstand: new, got a return"),
        }
    }
}

/// What the bootstrap leaves in the caches. The boot builds only the
/// kernel and `PlatformFactory`; the other shipped modules are built on a
/// program's first demand, so none of their slot initializers or
/// superclass clauses runs here, which is why the counts are well below
/// what a boot building every module left. Each eager slot initializer of
/// `PlatformFactory` that sends to the kernel instance takes a site and
/// fills the global entry of the class it names: its seventeen, `kernel
/// Exception` to `kernel WeakMap`, fill seventeen. The platform's boot
/// sends `PlatformFactory`'s `build`, `^Platform new`, whose family takes
/// two sites and fills two global entries (the lexical `Platform`, and
/// `new` to the class), and `Platform`'s superclass clause, `Value`, whose
/// family takes two more and whose lexical `Value` fills another. The boot
/// builds every class the two roots declare, so the superclass clause of
/// every one that is no builtin runs too -- `StringBuilder`'s,
/// `DataView`'s and `WeakMap`'s `Object` and its two byte orders'
/// `DataView` in the kernel, and `KernelNamespace`'s `Value` in
/// `PlatformFactory` -- and they take the remaining six sites and fill
/// the remaining two global entries, the kernel's lexical `Object` and
/// `DataView`, a lexical send being keyed by the module it is sent in, so
/// `KernelNamespace`'s `Value` finds `Platform`'s entry filled. Pinned so
/// that the boot growing -- more shipped code run at every start -- is
/// noticed rather than absorbed by the tests that count from it. The
/// cache stress mode vacates the caches at the boot's safepoints, which
/// may empty the global entries, so those are counted only without it;
/// the sites stay either way.
#[test]
fn a_fresh_bootstrap_leaves_twenty_seven_sites_and_twenty_two_global_entries() {
    let mut interpreter = Interpreter::bootstrap();
    let statistics = interpreter.cache_statistics();
    assert_eq!(statistics.site_entries, 27);
    let stress_caches = std::env::var("NS_NEXT_STRESS_CACHES").is_ok_and(|value| value == "1");
    if !stress_caches {
        assert_eq!(statistics.filled_global_entries, 22);
    }
}

/// Every entry's machine-readable cell (`docs/design/execution.md`, "Inline
/// caches") agrees with the entry after programs that leave every kind of
/// site behind -- monomorphic, polymorphic and megamorphic public sites, a
/// `super` site under two applications, a private lexical site, slot
/// accessors -- and again after a full collection, which scrubs the
/// entries naming freed ids and drops the families nothing runs.
/// `collect_now` is a debug build's.
#[cfg(debug_assertions)]
#[test]
fn every_cell_agrees_with_its_entry() {
    let private_lexical = indoc! {"
        class Subject = Object new () (
          class Outer named: label = Object new (| name = label. |) (
            private secret = ( ^name )
            public class Inner = Object new () (
              public reveal = ( ^secret )
            )
          )
          public run = (
            | sum |
            sum:: 0.
            {Outer named: 1. Outer named: 2} do: [:each | sum:: sum + each Inner new reveal].
            ^sum
          )
        ) : ()
    "};
    for (source, selector, argument) in [
        (FIVE_KINDS, "run:", Some(3)),
        (FIVE_KINDS, "run:", Some(5)),
        (TWO_APPLICATIONS, "run", None),
        (private_lexical, "run", None),
    ] {
        let (mut interpreter, class) = load(source);
        let instance = instantiate(&mut interpreter, &class);
        let arguments: Vec<Root> = argument
            .map(|integer| integer_argument(&interpreter, integer))
            .into_iter()
            .collect();
        send(&mut interpreter, &instance, selector, &arguments);
        if let Err(message) = interpreter.verify_cells() {
            panic!("{selector} {argument:?}: {message}");
        }
        interpreter.collect_now();
        if let Err(message) = interpreter.verify_cells() {
            panic!("{selector} {argument:?}, after a collection: {message}");
        }
    }
}

/// An edit installed through a MirrorBuilder vacates every cell with every
/// entry, so that a compiled caller holding the cell of the edited method
/// reaches the new one: the `answer` site's cell reads vacant after the
/// install, and the next send fills it again with the new method's answer.
#[test]
fn an_install_vacates_every_cell() {
    let source = indoc! {"
        class Subject usingPlatform: platform = Object new (
          |
            ClassMirror = platform mirrors ClassMirror.
          |
        ) (
          class Answerer = Object new () (
            public answer = ( ^1 )
          )
          public answerer = ( ^Answerer new )
          public ask: answerer = ( ^answerer answer )
          public edit = (
            | builder |
            builder:: (ClassMirror reflecting: Answerer) mixin declaration asBuilder.
            builder instanceSide methods addFromSource: 'public answer = ( ^2 )'.
            builder install.
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let platform = interpreter.platform();
    let subject = send(&mut interpreter, &class, "usingPlatform:", &[platform]);
    let answerer = send(&mut interpreter, &subject, "answerer", &[]);
    let cells = |interpreter: &mut Interpreter| {
        interpreter.call_site_cells(
            &class,
            MethodSide::Instance,
            Symbol::intern("ask:"),
            Symbol::intern("answer"),
        )
    };
    let answer = send(
        &mut interpreter,
        &subject,
        "ask:",
        std::slice::from_ref(&answerer),
    );
    assert_eq!(small_integer(&mut interpreter, &answer), 1);
    let filled = cells(&mut interpreter);
    assert_eq!(filled.len(), 1);
    assert!(!filled[0].is_vacant(), "the send filled the site's cell");
    send(&mut interpreter, &subject, "edit", &[]);
    assert!(
        cells(&mut interpreter).iter().all(|cell| cell.is_vacant()),
        "the install vacated the site's cell"
    );
    if let Err(message) = interpreter.verify_cells() {
        panic!("after the install: {message}");
    }
    let answer = send(&mut interpreter, &subject, "ask:", &[answerer]);
    assert_eq!(small_integer(&mut interpreter, &answer), 2);
    let refilled = cells(&mut interpreter);
    assert!(!refilled[0].is_vacant(), "the next send fills it again");
}

/// `count` sends of `yourself` to `self`, chained: the body of a method
/// whose family takes `count` sites, so that its range, once freed, is a
/// hole the slide of the site ranges closes (`docs/design/dispatch.md`,
/// "Code families").
#[cfg(debug_assertions)]
fn yourself_chain(count: usize) -> String {
    format!("self{}", " yourself".repeat(count))
}

/// A module whose `Victim>>answer` is a family of a thousand sites, which
/// answers a block of its family, and whose `editVictim` replaces it
/// through a MirrorBuilder, so that the old version's range is freed by the
/// next full collection once no closure of it lives; `holdVictim` keeps
/// one in `held` until `release:` lets it go. `keep` stores a block of its
/// own family in `kept`, and `useKept:` runs it. `carry` edits the victim
/// and then runs `release:`, which loops, lets the held closure go and
/// loops again, a frame of `carry`'s family below it all along: the first
/// collection after the release slides both families down while both
/// frames are on the stack. Each of `release:` and `carry` sends
/// `yourself` only after that, from a site no earlier send filled.
#[cfg(debug_assertions)]
fn sliding_source() -> String {
    let victim = yourself_chain(1_000);
    formatdoc! {"
        class Subject usingPlatform: platform = Object new (
          |
            ClassMirror = platform mirrors ClassMirror.
            public kept ::= nil.
            held ::= nil.
          |
        ) (
          class Victim = Object new () (
            public answer = (
              {victim}.
              ^[:value | value]
            )
          )
          public victim = ( ^Victim new answer )
          public holdVictim = ( held:: Victim new answer )
          public keep = ( kept:: [:value | value yourself + 1]. ^self )
          public useKept: value = ( ^kept value: value )
          public editVictim = (
            | builder |
            builder:: (ClassMirror reflecting: Victim) mixin declaration asBuilder.
            builder instanceSide methods addFromSource: 'public answer = ( ^0 )'.
            builder install.
          )
          public release: count = (
            | index ::= 0. |
            [index < count] whileTrue: [index:: index + 1].
            held:: nil.
            [index < (count * 2)] whileTrue: [index:: index + 1].
            ^index yourself
          )
          public carry = (
            | total |
            self editVictim.
            total:: self release: 20000.
            ^total yourself + 1
          )
        ) : ()
    "}
}

/// `sliding_source` loaded under `lowering` and instantiated with the
/// platform, its `victim` run first, so that the victim's family takes the
/// low range every family run after it lies above.
#[cfg(debug_assertions)]
fn sliding_subject(lowering: Lowering) -> (Interpreter, Root, Root) {
    let (mut interpreter, class) = load_with(&sliding_source(), lowering);
    let platform = interpreter.platform();
    let subject = send(&mut interpreter, &class, "usingPlatform:", &[platform]);
    send(&mut interpreter, &subject, "victim", &[]);
    (interpreter, class, subject)
}

/// A module whose `churn:` replaces `Edited>>answer` `count` times through a
/// MirrorBuilder, each version a family of ten sites run once, and whose
/// `keeper` is a family of its own.
#[cfg(debug_assertions)]
const FAMILY_CHURN: &str = indoc! {"
    class Churn usingPlatform: platform = Object new (
      |
        ClassMirror = platform mirrors ClassMirror.
      |
    ) (
      class Edited = Object new () (
        public answer = ( ^0 )
      )
      public keeper = ( ^Edited new answer yourself yourself )
      public churn: count = (
        1 to: count do: [:index |
          | builder |
          builder:: (ClassMirror reflecting: Edited) mixin declaration asBuilder.
          builder instanceSide methods
              addFromSource: 'public answer = ( ^' , index printString , ' yourself yourself yourself yourself yourself yourself yourself yourself yourself yourself )'.
          builder install.
          Edited new answer = index ifFalse: [^false]].
        ^true
      )
    ) : ()
"};

/// `FAMILY_CHURN` run for `edits` edits and then its `keeper`, with no
/// collection meanwhile -- every floor and the collection floor far above
/// what the edits allocate, the collect stress mode off -- so that every
/// version but the last is still in the table when `collect_now` runs.
/// Answers the interpreter, the module's class and its instance.
#[cfg(debug_assertions)]
fn churned(edits: i64) -> (Interpreter, Root, Root) {
    churned_optimizing(edits, false)
}

/// `churned`, with the optimize stress mode on when `optimizing` says so:
/// every body's first spend requests its optimization, so that `churn:`
/// runs in a copy that inlines each version of `answer`, which every edit
/// frees.
#[cfg(debug_assertions)]
fn churned_optimizing(edits: i64, optimizing: bool) -> (Interpreter, Root, Root) {
    let (mut interpreter, class) = load(FAMILY_CHURN);
    if optimizing {
        interpreter.set_optimize_stress(true);
    }
    interpreter.set_stress_collect(false);
    interpreter.set_collection_floor(1 << 30);
    interpreter.set_table_floors(ns_runtime::TableFloors::all(1 << 24));
    let platform = interpreter.platform();
    let churn = send(&mut interpreter, &class, "usingPlatform:", &[platform]);
    let count = integer_argument(&interpreter, edits);
    let answered = send(&mut interpreter, &churn, "churn:", &[count]);
    let answered_true = interpreter
        .enter(|mutator| matches!(mutator.fetch(&answered).kind(), ValueKind::Boolean(true)));
    assert!(answered_true, "an edited method answered another value");
    let kept = send(&mut interpreter, &churn, "keeper", &[]);
    assert_eq!(small_integer(&mut interpreter, &kept), edits);
    (interpreter, class, churn)
}

/// A churn of families keeps the site table bounded: two thousand edits,
/// each version's family run once and dead at the next collection, leave
/// a hole of nearly twenty thousand sites below `keeper`, the long-lived
/// family run last, at the table's tail; the full collection slides the
/// live ranges down over it, so that the table is at most twice the sites
/// of the same families after a single edit. `keeper`'s entries move with
/// its range, so that its next run hits them where its family now lies.
#[cfg(debug_assertions)]
#[test]
fn a_churn_of_families_keeps_the_site_table_bounded() {
    let (mut once, _, _) = churned(1);
    once.collect_now();
    let live = once.table_sizes().sites;
    let (mut interpreter, class, churn) = churned(2_000);
    let before = interpreter.cache_statistics();
    interpreter.collect_now();
    let after = interpreter.cache_statistics();
    let sites = interpreter.table_sizes().sites;
    assert!(
        sites <= 2 * live,
        "{sites} sites after the churn, {live} after a single edit"
    );
    assert!(after.site_slides > before.site_slides, "the ranges slid");
    let filled = states(
        &mut interpreter,
        &class,
        MethodSide::Instance,
        "keeper",
        "yourself",
    );
    assert_eq!(filled.len(), 2);
    assert!(
        filled.iter().all(|state| *state != CallSiteState::Vacant),
        "keeper's entries moved with its range: {filled:?}"
    );
    let kept = send(&mut interpreter, &churn, "keeper", &[]);
    assert_eq!(small_integer(&mut interpreter, &kept), 2_000);
    if let Err(message) = interpreter.verify_cells() {
        panic!("after the slide: {message}");
    }
}

/// The churn of `a_churn_of_families_keeps_the_site_table_bounded` run in
/// optimized code: each edit frees the copy of `churn:` that inlined the
/// version it replaced, and a freed copy keeps nothing it inlined, so that
/// the full collection ends every replaced version's family as it does in
/// baseline code and the interpreter.
#[cfg(debug_assertions)]
#[test]
fn a_churn_of_families_in_optimized_code_keeps_the_site_table_bounded() {
    let (mut once, _, _) = churned_optimizing(1, true);
    once.collect_now();
    let live = once.table_sizes().sites;
    let (mut interpreter, _, churn) = churned_optimizing(2_000, true);
    interpreter.collect_now();
    let sites = interpreter.table_sizes().sites;
    assert!(
        sites <= 2 * live,
        "{sites} sites after the churn, {live} after a single edit"
    );
    let kept = send(&mut interpreter, &churn, "keeper", &[]);
    assert_eq!(small_integer(&mut interpreter, &kept), 2_000);
}

/// The churn of `a_churn_of_families_keeps_the_site_table_bounded`, with
/// the cells' shrink pair lowered to 64 KiB and 32 KiB: the slide leaves
/// the site table a fraction of what the churn grew it to, and the
/// collection gives the cells' pages above it back
/// (`heap::limits::Shrink`); `keeper` still hits its moved entries, and
/// every cell agrees with its entry.
#[cfg(debug_assertions)]
#[test]
fn the_cells_shrink_after_a_slide() {
    let (mut interpreter, _, churn) = churned(2_000);
    interpreter.set_shrink_pairs(ns_runtime::ShrinkPairs {
        cells: ns_runtime::Shrink {
            threshold: 64 << 10,
            headroom: 32 << 10,
        },
        // The band's end, not the damping's ramp, is what this pins.
        damped: false,
        ..ns_runtime::ShrinkPairs::default()
    });
    let before = interpreter.table_sizes().cells_committed_bytes;
    let slides = interpreter.cache_statistics().site_slides;
    interpreter.collect_now();
    // The first collection sees the churn's growth, demand, and keeps the
    // cells; the next, after none, gives them back.
    interpreter.collect_now();
    assert!(
        interpreter.cache_statistics().site_slides > slides,
        "the ranges slid"
    );
    let after = interpreter.table_sizes().cells_committed_bytes;
    assert!(
        after < before,
        "{after} bytes of cells committed after the slide, {before} before"
    );
    let kept = send(&mut interpreter, &churn, "keeper", &[]);
    assert_eq!(small_integer(&mut interpreter, &kept), 2_000);
    if let Err(message) = interpreter.verify_cells() {
        panic!("after the shrink: {message}");
    }
}

ns_runtime::for_each_lowering! {
    /// A closure made before a slide, stored in a slot, runs at its
    /// family's moved range after it: the collection rewrote its feedback
    /// base with the family's, so that its sends fill the sites the family
    /// now names, which the states read. Under the reference lowering every
    /// send runs uncached and no family takes a range, so only the answers
    /// are pinned there. `collect_now` is a debug build's.
    #[cfg(debug_assertions)]
    fn a_closure_made_before_a_slide_hits_at_its_moved_range(lowering) {
        let (mut interpreter, class, subject) = sliding_subject(lowering);
        interpreter.set_stress_caches(false);
        send(&mut interpreter, &subject, "keep", &[]);
        let one = integer_argument(&interpreter, 1);
        let answer = send(&mut interpreter, &subject, "useKept:", &[one]);
        assert_eq!(small_integer(&mut interpreter, &answer), 2);
        let closure = send(&mut interpreter, &subject, "kept", &[]);
        let before = interpreter
            .closure_feedback_base(&closure)
            .expect("kept holds a closure");
        // Counted from before the edit: the collect stress mode may slide
        // the ranges inside it, before `collect_now`.
        let slides = interpreter.cache_statistics().site_slides;
        send(&mut interpreter, &subject, "editVictim", &[]);
        interpreter.collect_now();
        let after = interpreter
            .closure_feedback_base(&closure)
            .expect("kept holds a closure");
        let two = integer_argument(&interpreter, 2);
        let answer = send(&mut interpreter, &subject, "useKept:", &[two]);
        assert_eq!(small_integer(&mut interpreter, &answer), 3);
        if lowering == Lowering::Optimized {
            assert!(
                interpreter.cache_statistics().site_slides > slides,
                "the collection slid the ranges"
            );
            assert!(
                after < before,
                "the closure's base moved down with its family, from {before} to {after}"
            );
            let filled = states(&mut interpreter, &class, MethodSide::Instance, "keep", "yourself");
            assert_eq!(filled.len(), 1);
            assert_ne!(
                filled[0],
                CallSiteState::Vacant,
                "the closure filled its site at the moved range"
            );
            if let Err(message) = interpreter.verify_cells() {
                panic!("after the slide: {message}");
            }
        }
    }
}

/// `carry` run with the collect stress mode on and, with `suspend`, the
/// suspend stress mode, the cache stress mode off so that the states read
/// what the run filled: answers what `carry` answered, how many slides
/// the run made and how many times the suspend stress mode suspended.
#[cfg(debug_assertions)]
fn carried(suspend: bool) -> (Interpreter, Root, i64, u64, usize) {
    let (mut interpreter, class, subject) = sliding_subject(Lowering::Optimized);
    interpreter.set_stress_caches(false);
    send(&mut interpreter, &subject, "holdVictim", &[]);
    let slides = interpreter.cache_statistics().site_slides;
    interpreter.set_stress_suspend(suspend);
    interpreter.set_stress_collect(true);
    let answer = send(&mut interpreter, &subject, "carry", &[]);
    let answer = small_integer(&mut interpreter, &answer);
    let suspensions = interpreter.stress_suspensions();
    interpreter.set_stress_collect(false);
    interpreter.set_stress_suspend(false);
    let slid = interpreter.cache_statistics().site_slides - slides;
    (interpreter, class, answer, slid, suspensions)
}

/// Whether the one `yourself` site of `method`, sent only after the slide,
/// was filled: at the family's moved range, where the states read it.
#[cfg(debug_assertions)]
fn filled_after_the_slide(interpreter: &mut Interpreter, class: &Root, method: &str) -> bool {
    let filled = states(interpreter, class, MethodSide::Instance, method, "yourself");
    assert_eq!(filled.len(), 1, "{method} sends `yourself` once");
    filled[0] != CallSiteState::Vacant
}

/// Frames running across a slide hit at their families' moved range, with
/// no suspension to derive their bases again: `release:` is the running
/// frame when the collection after its release of the held closure frees
/// the victim's old range and slides both families down, and `carry`'s
/// frame lies below it. `release:`'s next send takes the base the run loop
/// read again from its frame after the safepoint, and `carry`'s, once
/// `release:` returns, the base the collection rewrote in its header; a
/// stale one would fill a site of another range, and the moved one would
/// stay vacant.
#[cfg(debug_assertions)]
#[test]
fn a_running_frame_hits_at_its_moved_range_after_a_slide() {
    let (mut interpreter, class, answer, slid, _) = carried(false);
    assert_eq!(answer, 40_001);
    assert!(slid > 0, "a collection slid the ranges");
    assert!(
        filled_after_the_slide(&mut interpreter, &class, "release:"),
        "the running frame filled its site at the moved range"
    );
    assert!(
        filled_after_the_slide(&mut interpreter, &class, "carry"),
        "the frame below it filled its site at the moved range"
    );
    if let Err(message) = interpreter.verify_cells() {
        panic!("after the run: {message}");
    }
}

/// A frame suspended across a slide resumes at its family's moved range:
/// `carry`'s frame lies below `release:`, whose first loop runs as many
/// safepoints as the second, so that the suspend stress mode, paced as the
/// collect stress mode is, has suspended it before the release lets the
/// victim's old range go, and keeps it suspended until `release:` returns,
/// across the slide the release allows. The resumed frame derives its base
/// again, and its send of `yourself` fills the site its family now names.
#[cfg(debug_assertions)]
#[test]
fn a_suspended_frame_resumes_at_its_moved_range() {
    let (mut interpreter, class, answer, slid, suspensions) = carried(true);
    assert_eq!(answer, 40_001);
    assert!(suspensions > 0, "the run suspended frames");
    assert!(slid > 0, "a collection slid the ranges");
    assert!(
        filled_after_the_slide(&mut interpreter, &class, "carry"),
        "the resumed frame filled its site at the moved range"
    );
    if let Err(message) = interpreter.verify_cells() {
        panic!("after the run: {message}");
    }
}
