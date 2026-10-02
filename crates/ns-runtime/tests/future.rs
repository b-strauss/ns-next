//! The pull future of §6.3.2's simultaneous slots, driven from the host
//! before any source makes one (`docs/design/object-model.md`,
//! "Simultaneous slots"). Each test runs under both lowerings: the
//! future's `doesNotUnderstand:` forces it through a mirror send, which the
//! optimized lowering inlines as `SendMirror` and the reference lowering
//! sends to the mirror object.

use indoc::indoc;
use ns_runtime::interpreter::future::FutureState;
use ns_runtime::{HostResult, HostSignal, Interpreter, Lowering, Root, Symbol, Value, ValueKind};

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// The error message a host send ended in; a panic if it answered.
fn expect_error_message(result: HostResult, what: &str) -> String {
    match result {
        Err(HostSignal::Error(error)) => error.message,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Ok(_) => panic!("{what}: answered, where it should have ended in an error"),
    }
}

const HOLDER: &str = indoc! {"
    class Holder = Object new (
        | public runs ::= 0. public held |
    ) (
        public three = ( ^[runs: runs + 1. 3] )
        public failing = ( ^[runs: runs + 1. nil zork] )
        public looping = ( ^[held + 1] )
        public answeringHeld = ( ^[held] )
        public taking = ( ^[:argument | argument] )
        public answering: answer = ( ^[answer] )
        public descend: depth forcing: future = (
            depth = 0 ifTrue: [^future + 1].
            ^descend: depth - 1 forcing: future
        )
    ) : ()
"};

/// A `Holder`, whose methods answer blocks to make futures of and count how
/// often the counting blocks ran, in an interpreter compiling with
/// `lowering`.
fn a_holder(lowering: Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(HOLDER).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        HOLDER.into(),
        ns_runtime::SourceOrigin::File("holder.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    let holder = expect_ok(interpreter.instantiate(&class, &[]), "Holder new");
    (interpreter, holder)
}

/// A future of the block `holder` answers for `selector`.
fn a_future(interpreter: &mut Interpreter, holder: &Root, selector: &str) -> Root {
    let block = expect_ok(
        interpreter.send(holder, Symbol::intern(selector), &[]),
        selector,
    );
    interpreter.enter(|mutator| {
        let Some(closure) = mutator.fetch(&block).as_object() else {
            panic!("{selector} answers a block");
        };
        let future = mutator.new_future(closure);
        mutator.stash(Value::object(future))
    })
}

/// How many times the holder's counting blocks ran.
fn runs(interpreter: &mut Interpreter, holder: &Root) -> i64 {
    let runs = expect_ok(
        interpreter.send(holder, Symbol::intern("runs"), &[]),
        "runs",
    );
    interpreter.enter(|mutator| match mutator.fetch(&runs).kind() {
        ValueKind::SmallInteger(count) => count,
        _ => panic!("runs is an integer"),
    })
}

/// Which of its states `future` is in, as a word, since a
/// `FutureState` carries handles that cannot leave the entry.
fn state_of(interpreter: &mut Interpreter, future: &Root) -> &'static str {
    interpreter.enter(|mutator| {
        let Some(future) = mutator.fetch(future).as_object() else {
            panic!("a future is an object");
        };
        match mutator.future_state(future) {
            FutureState::Unresolved(_) => "unresolved",
            FutureState::Resolving => "resolving",
            FutureState::Resolved(_) => "resolved",
            FutureState::Malformed => "malformed",
        }
    })
}

ns_runtime::for_each_lowering! {
    fn a_message_to_a_future_is_answered_by_its_value_computed_once(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let future = a_future(&mut interpreter, &holder, "three");
        let four = interpreter.stash(Value::small_integer(4));
        let sum = expect_ok(
            interpreter.send(&future, Symbol::intern("+"), std::slice::from_ref(&four)),
            "future + 4",
        );
        let again = expect_ok(
            interpreter.send(&future, Symbol::intern("+"), &[four]),
            "future + 4 again",
        );
        interpreter.enter(|mutator| {
            assert_eq!(mutator.fetch(&sum).as_small_integer(), Some(7));
            assert_eq!(mutator.fetch(&again).as_small_integer(), Some(7));
        });
        assert_eq!(runs(&mut interpreter, &holder), 1, "the block ran once");
        assert_eq!(state_of(&mut interpreter, &future), "resolved");
    }
}

ns_runtime::for_each_lowering! {
    /// §5.5: every public member of `Object` is protected in `Future`, so
    /// even `class`, `==` and `printString` reach `doesNotUnderstand:` and
    /// are answered by the value.
    fn a_future_answers_objects_own_messages_as_its_value(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let future = a_future(&mut interpreter, &holder, "three");
        let class = expect_ok(
            interpreter.send(&future, Symbol::intern("class"), &[]),
            "future class",
        );
        let printed = expect_ok(
            interpreter.send(&future, Symbol::intern("printString"), &[]),
            "printString",
        );
        let three = interpreter.stash(Value::small_integer(3));
        let identical = expect_ok(
            interpreter.send(&future, Symbol::intern("=="), &[three]),
            "future == 3",
        );
        let is_nil = expect_ok(
            interpreter.send(&future, Symbol::intern("isNil"), &[]),
            "future isNil",
        );
        interpreter.enter(|mutator| {
            let Some(class) = mutator.fetch(&class).as_object() else {
                panic!("a class")
            };
            assert!(class.same(mutator.builtin_class(ns_runtime::BuiltinClass::Integer)));
            assert_eq!(
                mutator.heap().describe(&mutator.fetch(&printed)),
                "the String \"3\""
            );
            assert_eq!(mutator.fetch(&identical).as_boolean(), Some(true));
            assert_eq!(mutator.fetch(&is_nil).as_boolean(), Some(false));
        });
        assert_eq!(runs(&mut interpreter, &holder), 1, "the block ran once");
    }
}

/// Every public selector `Object` declares is overridden by `Future` as
/// protected, so a member added to `Object` later without its override
/// fails here rather than letting a future tell itself from its value.
#[test]
fn future_overrides_every_public_member_of_object_as_protected() {
    let mut interpreter = Interpreter::bootstrap();
    interpreter.enter(|mutator| {
        let public_of_object = mutator.public_selectors_of(ns_runtime::BuiltinClass::Object);
        assert!(
            !public_of_object.is_empty(),
            "Object declares public members"
        );
        for selector in public_of_object {
            assert_eq!(
                mutator.access_of(ns_runtime::BuiltinClass::Future, selector),
                Some(ns_syntax::ast::AccessModifier::Protected),
                "Future must override {selector} as protected"
            );
        }
    });
}

ns_runtime::for_each_lowering! {
    /// A future forced while its own block runs is §6.3.2's ill-founded
    /// recursion, an ordinary error; the forcing it abandoned leaves the
    /// future unresolved.
    fn forcing_a_future_that_is_being_computed_is_an_error(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        // `held` is set to the future of `[held + 1]`, which forces itself.
        let future = a_future(&mut interpreter, &holder, "looping");
        let _ = expect_ok(
            interpreter.send(&holder, Symbol::intern("held:"), std::slice::from_ref(&future)),
            "held:",
        );
        let one = interpreter.stash(Value::small_integer(1));
        let message = expect_error_message(
            interpreter.send(&future, Symbol::intern("+"), &[one]),
            "a future forcing itself",
        );
        assert!(message.contains("divergent evaluation"), "{message}");
        assert_eq!(state_of(&mut interpreter, &future), "unresolved");
    }
}

ns_runtime::for_each_lowering! {
    /// An abandoned forcing -- its computation raised and something above
    /// caught it -- leaves the future unresolved, not stuck resolving, so
    /// the next touch computes again and raises the same error.
    fn abandoning_a_forcing_resets_the_future(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let future = a_future(&mut interpreter, &holder, "failing");
        let one = interpreter.stash(Value::small_integer(1));
        for attempt in 0..2 {
            let message = expect_error_message(
                interpreter.send(&future, Symbol::intern("+"), std::slice::from_ref(&one)),
                "the failing future",
            );
            assert!(message.contains("zork"), "attempt {attempt}: {message}");
            assert_eq!(state_of(&mut interpreter, &future), "unresolved");
        }
        assert_eq!(runs(&mut interpreter, &holder), 2, "computed again");
    }
}

ns_runtime::for_each_lowering! {
    /// A future whose block answers another, unresolved future is resolved
    /// to that one's value, and resolves it on the way: §6.3.2's pipelined
    /// promise.
    fn a_future_answering_another_future_resolves_both(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let inner = a_future(&mut interpreter, &holder, "three");
        let _ = expect_ok(
            interpreter.send(&holder, Symbol::intern("held:"), std::slice::from_ref(&inner)),
            "held:",
        );
        let outer = a_future(&mut interpreter, &holder, "answeringHeld");
        let four = interpreter.stash(Value::small_integer(4));
        let sum = expect_ok(
            interpreter.send(&outer, Symbol::intern("+"), &[four]),
            "outer + 4",
        );
        interpreter.enter(|mutator| {
            assert_eq!(mutator.fetch(&sum).as_small_integer(), Some(7));
            let Some(outer) = mutator.fetch(&outer).as_object() else {
                panic!("a future is an object");
            };
            // Resolved to the value, never to the inner future.
            assert!(matches!(
                mutator.future_state(outer),
                FutureState::Resolved(resolved) if resolved.as_small_integer() == Some(3)
            ));
        });
        assert_eq!(state_of(&mut interpreter, &inner), "resolved");
        assert_eq!(runs(&mut interpreter, &holder), 1, "the inner block ran once");
    }
}

ns_runtime::for_each_lowering! {
    /// When the inner future of such a chain raises, both forcings are
    /// abandoned: the outer one, whose activation had already returned,
    /// is unresolved again too.
    fn abandoning_a_chain_resets_every_future_in_it(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let inner = a_future(&mut interpreter, &holder, "failing");
        let _ = expect_ok(
            interpreter.send(&holder, Symbol::intern("held:"), std::slice::from_ref(&inner)),
            "held:",
        );
        let outer = a_future(&mut interpreter, &holder, "answeringHeld");
        let one = interpreter.stash(Value::small_integer(1));
        let message = expect_error_message(
            interpreter.send(&outer, Symbol::intern("+"), &[one]),
            "the chain",
        );
        assert!(message.contains("zork"), "{message}");
        assert_eq!(state_of(&mut interpreter, &inner), "unresolved");
        assert_eq!(state_of(&mut interpreter, &outer), "unresolved");
    }
}

ns_runtime::for_each_lowering! {
    /// A forcing whose block the run loop refuses to activate -- here for
    /// its arity; the depth limit refuses the same way -- abandons the
    /// forcing, so the future is not left resolving with nothing computing
    /// it.
    fn a_refused_forcing_leaves_the_future_unresolved(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let future = a_future(&mut interpreter, &holder, "taking");
        let one = interpreter.stash(Value::small_integer(1));
        for attempt in 0..2 {
            let _ = expect_error_message(
                interpreter.send(&future, Symbol::intern("+"), std::slice::from_ref(&one)),
                "a future of a one-argument block",
            );
            assert_eq!(state_of(&mut interpreter, &future), "unresolved", "attempt {attempt}");
        }
    }
}

ns_runtime::for_each_lowering! {
    /// The depth limit refuses a forcing's push as it refuses any other
    /// (`Mutator::push_past_the_limit`), and abandons the forcing, so the
    /// future is unresolved, not resolving with nothing computing it. The
    /// forcing's push is the one refused when the error names a block's
    /// `value`: `descend:forcing:` and the future's `doesNotUnderstand:` are
    /// methods. Searched from the deepest recursion down, since the first
    /// depth that fits the forcing resolves the future.
    fn the_depth_limit_refusing_a_forcing_leaves_the_future_unresolved(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let future = a_future(&mut interpreter, &holder, "three");
        let limit = i64::try_from(ns_runtime::interpreter::MAX_ACTIVATION_DEPTH)
            .expect("the limit is a small integer");
        let mut refused_forcing = false;
        for depth in (limit - 10..limit).rev() {
            let arguments = [
                interpreter.stash(Value::small_integer(depth)),
                future.clone(),
            ];
            let error = match interpreter.send(
                &holder,
                Symbol::intern("descend:forcing:"),
                &arguments,
            ) {
                Err(HostSignal::Error(error)) => error,
                Err(HostSignal::Return) => panic!("depth {depth}: a non-local return escaped"),
                Ok(_) => break,
            };
            assert_eq!(error.message, "stack overflow: recursion too deep", "depth {depth}");
            if error.selector == Symbol::intern("value") {
                refused_forcing = true;
                assert_eq!(state_of(&mut interpreter, &future), "unresolved", "depth {depth}");
                assert_eq!(runs(&mut interpreter, &holder), 0, "depth {depth}: never ran");
                break;
            }
        }
        assert!(refused_forcing, "{lowering:?}: some depth refused the forcing's push");
        // The next touch computes it.
        let four = interpreter.stash(Value::small_integer(4));
        let sum = expect_ok(
            interpreter.send(&future, Symbol::intern("+"), &[four]),
            "future + 4",
        );
        interpreter.enter(|mutator| {
            assert_eq!(mutator.fetch(&sum).as_small_integer(), Some(7));
        });
        assert_eq!(state_of(&mut interpreter, &future), "resolved");
    }
}

/// Only an instance of the kernel's `Future` is a future.
#[test]
fn only_a_future_is_a_future() {
    let (mut interpreter, holder) = a_holder(Lowering::Optimized);
    let future = a_future(&mut interpreter, &holder, "three");
    interpreter.enter(|mutator| {
        let future = mutator.fetch(&future);
        assert!(mutator.as_future(future).is_some());
        assert!(mutator.as_future(mutator.fetch(&holder)).is_none());
        assert!(mutator.as_future(Value::small_integer(3)).is_none());
        assert!(mutator.as_future(Value::nil()).is_none());
    });
}

ns_runtime::for_each_lowering! {
    /// A host's send has no instruction to run again, so an unresolved future
    /// it passes straight to a leaf ends the run with an error rather than
    /// answering wrongly (`docs/design/object-model.md`, "Where Rust sees a
    /// future").
    fn a_host_passing_an_unresolved_future_to_a_leaf_is_an_error(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let future = a_future(&mut interpreter, &holder, "three");
        let one = interpreter.stash(Value::small_integer(1));
        let message = expect_error_message(
            interpreter.send(&one, Symbol::intern("+"), &[future]),
            "1 + an unresolved future from the host",
        );
        assert!(message.contains("host"), "{message}");
    }
}

ns_runtime::for_each_lowering! {
    /// A resolved future needs no instruction run again: the leaf sees its
    /// value in Rust, so even a host's send is answered.
    fn a_host_passing_a_resolved_future_to_a_leaf_is_answered_by_its_value(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let future = a_future(&mut interpreter, &holder, "three");
        let four = interpreter.stash(Value::small_integer(4));
        expect_ok(
            interpreter.send(&future, Symbol::intern("+"), &[four]),
            "future + 4, which resolves it",
        );
        let one = interpreter.stash(Value::small_integer(1));
        let sum = expect_ok(
            interpreter.send(&one, Symbol::intern("+"), &[future]),
            "1 + a resolved future",
        );
        interpreter.enter(|mutator| {
            assert_eq!(mutator.fetch(&sum).as_small_integer(), Some(4));
        });
    }
}

ns_runtime::for_each_lowering! {
    /// No program names `Future`: `Object` declares no accessor for it, so a
    /// bare `Future` is a message nothing understands, and neither does
    /// `platform kernel`, which hands out only the exception classes and
    /// `Message`.
    fn no_program_names_the_future_class(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let message = expect_error_message(
            interpreter.send(&holder, Symbol::intern("Future"), &[]),
            "Holder Future",
        );
        assert!(message.contains("does not understand #Future"), "{message}");
        let platform = interpreter.platform();
        let kernel = expect_ok(
            interpreter.send(&platform, Symbol::intern("kernel"), &[]),
            "platform kernel",
        );
        let message = expect_error_message(
            interpreter.send(&kernel, Symbol::intern("Future"), &[]),
            "platform kernel Future",
        );
        assert!(message.contains("does not understand #Future"), "{message}");
    }
}

ns_runtime::for_each_lowering! {
    /// A chain of futures each answering the next is as long as the
    /// program makes it, and the run loop pops each block's activation
    /// before it pushes the next, so no depth limit bounds it: resolving,
    /// abandoning, tracing and dropping the chain must all walk it without
    /// recursing in Rust once per link.
    fn a_long_chain_of_futures_resolves_without_recursing_per_link(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let next = a_chain(&mut interpreter, &holder, "three");
        let four = interpreter.stash(Value::small_integer(4));
        let sum = expect_ok(
            interpreter.send(&next, Symbol::intern("+"), &[four]),
            "head + 4",
        );
        interpreter.enter(|mutator| {
            assert_eq!(mutator.fetch(&sum).as_small_integer(), Some(7));
        });
        assert_eq!(state_of(&mut interpreter, &next), "resolved");
        assert_eq!(runs(&mut interpreter, &holder), 1, "the last block ran once");
    }
}

ns_runtime::for_each_lowering! {
    /// The same chain, its last block raising: the unwind abandons the
    /// whole chain, one link at a time, and leaves the head unresolved.
    fn a_long_chain_of_futures_is_abandoned_without_recursing_per_link(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let next = a_chain(&mut interpreter, &holder, "failing");
        let one = interpreter.stash(Value::small_integer(1));
        let message = expect_error_message(
            interpreter.send(&next, Symbol::intern("+"), &[one]),
            "head + 1",
        );
        assert!(message.contains("zork"), "{message}");
        assert_eq!(state_of(&mut interpreter, &next), "unresolved");
    }
}

/// How many futures `a_chain` links: far more than any native stack holds
/// frames of a recursion per link.
const LINKS: usize = 200_000;

/// The head of `LINKS` futures, each one's block answering the next, the
/// last one's the block `holder` answers for `last`.
fn a_chain(interpreter: &mut Interpreter, holder: &Root, last: &str) -> Root {
    let mut next = a_future(interpreter, holder, last);
    for _ in 1..LINKS {
        let block = expect_ok(
            interpreter.send(holder, Symbol::intern("answering:"), &[next]),
            "answering:",
        );
        next = interpreter.enter(|mutator| {
            let Some(closure) = mutator.fetch(&block).as_object() else {
                panic!("answering: answers a block");
            };
            let future = mutator.new_future(closure);
            mutator.stash(Value::object(future))
        });
    }
    next
}

ns_runtime::for_each_lowering! {
    /// A collection replaces a resolved future with its value in every
    /// object field (`docs/design/memory.md`, "A resolved future is traced
    /// as its value"), so an array a future escaped into holds the value
    /// itself afterwards -- and the future, reachable from nothing else, is
    /// collected. Debug builds only: `collect_now` is a test hook of theirs.
    #[cfg(debug_assertions)]
    fn a_collection_replaces_a_resolved_future_in_an_array(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let future = a_future(&mut interpreter, &holder, "three");
        let four = interpreter.stash(Value::small_integer(4));
        let _ = expect_ok(interpreter.send(&future, Symbol::intern("+"), &[four]), "force it");
        let array = interpreter.enter(|mutator| {
            let future = mutator.fetch(&future);
            let array = mutator.new_array(vec![future]);
            mutator.stash(Value::object(array))
        });
        // A first collection, with the future still rooted, gives back
        // whatever else the setup left behind, so the second one's count
        // falls only by what dropping the future's root released.
        interpreter.collect_now();
        let while_rooted = interpreter.collector_statistics().live_objects;
        drop(future);
        interpreter.collect_now();
        assert!(
            interpreter.collector_statistics().live_objects < while_rooted,
            "the future, reachable from nothing once its root is dropped, is collected"
        );
        interpreter.enter(|mutator| {
            let Some(array) = mutator.fetch(&array).as_object() else {
                panic!("an array");
            };
            assert_eq!(mutator.heap().array_at(array, 0).and_then(Value::as_small_integer), Some(3));
        });
    }
}

ns_runtime::for_each_lowering! {
    /// The same replacement in an instance's slot: `held` holds the value
    /// itself once a collection has run.
    #[cfg(debug_assertions)]
    fn a_collection_replaces_a_resolved_future_in_an_instance_slot(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let future = a_future(&mut interpreter, &holder, "three");
        let four = interpreter.stash(Value::small_integer(4));
        let _ = expect_ok(interpreter.send(&future, Symbol::intern("+"), &[four]), "force it");
        let _ = expect_ok(
            interpreter.send(&holder, Symbol::intern("held:"), std::slice::from_ref(&future)),
            "held:",
        );
        interpreter.enter(|mutator| {
            let Some(holder) = mutator.fetch(&holder).as_object() else {
                panic!("an instance");
            };
            // Or the value already, where a stress mode collected since.
            let held = mutator.heap().instance_field(holder, 1).expect("held");
            assert!(
                mutator.as_future(held).is_some() || held.as_small_integer() == Some(3),
                "held holds the future first"
            );
        });
        drop(future);
        interpreter.collect_now();
        interpreter.enter(|mutator| {
            let Some(holder) = mutator.fetch(&holder).as_object() else {
                panic!("an instance");
            };
            // `held` is `Holder`'s second slot, after `runs`.
            assert!(
                mutator.heap().instance_field(holder, 1).and_then(Value::as_small_integer) == Some(3),
                "held holds the value, not the future"
            );
        });
    }
}

/// And in a closure's copied values: `answering:`'s block copies its
/// argument under the optimized lowering, which is the lowering that copies
/// -- the reference lowering reaches it through the captured frame, whose
/// registers are exempt (`Heap::replace_resolved_futures`). Debug builds
/// only, as above.
#[cfg(debug_assertions)]
#[test]
fn a_collection_replaces_a_resolved_future_in_a_closures_copied_value() {
    let (mut interpreter, holder) = a_holder(Lowering::Optimized);
    let future = a_future(&mut interpreter, &holder, "three");
    let four = interpreter.stash(Value::small_integer(4));
    let _ = expect_ok(
        interpreter.send(&future, Symbol::intern("+"), &[four]),
        "force it",
    );
    let block = expect_ok(
        interpreter.send(
            &holder,
            Symbol::intern("answering:"),
            std::slice::from_ref(&future),
        ),
        "answering:",
    );
    let copied_first = |interpreter: &mut Interpreter| {
        interpreter.enter(|mutator| {
            let Some(closure) = mutator.fetch(&block).as_object() else {
                panic!("answering: answers a block");
            };
            let data = mutator.heap().closure_data(closure).expect("a closure");
            match data.copied.first() {
                Some(value) if value.as_small_integer() == Some(3) => "the value",
                Some(value) if mutator.as_future(*value).is_some() => "the future",
                _ => "something else",
            }
        })
    };
    // Or the value already, where a stress mode collected since.
    assert_ne!(
        copied_first(&mut interpreter),
        "something else",
        "copied before a collection"
    );
    drop(future);
    interpreter.collect_now();
    assert_eq!(
        copied_first(&mut interpreter),
        "the value",
        "copied after a collection"
    );
    let answer = expect_ok(
        interpreter.send(&block, Symbol::intern("value"), &[]),
        "block value",
    );
    interpreter.enter(|mutator| {
        assert_eq!(mutator.fetch(&answer).as_small_integer(), Some(3));
    });
}
