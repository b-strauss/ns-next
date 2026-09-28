//! The pull future of §6.3.2's simultaneous slots, driven from the host
//! before any source makes one (`docs/design/object-model.md`,
//! "Simultaneous slots"). Each test runs under both lowerings: the
//! future's `doesNotUnderstand:` forces it through a mirror send, which the
//! optimized lowering inlines as `SendMirror` and the reference lowering
//! sends to the mirror object.

use indoc::indoc;
use ns_runtime::interpreter::future::FutureState;
use ns_runtime::{HostResult, HostSignal, Interpreter, Lowering, Root, Symbol, Value};

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
    Newspeak3
    'Test'
    class Holder = Object new (
        | public runs ::= 0. public held |
    ) (
        public three = ( ^[runs: runs + 1. 3] )
        public failing = ( ^[runs: runs + 1. nil zork] )
        public looping = ( ^[held + 1] )
        public answeringHeld = ( ^[held] )
        public taking = ( ^[:argument | argument] )
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
        let Value::Object(closure) = mutator.fetch(&block) else {
            panic!("{selector} answers a block");
        };
        let future = mutator.new_future(closure);
        mutator.stash(Value::Object(future))
    })
}

/// How many times the holder's counting blocks ran.
fn runs(interpreter: &mut Interpreter, holder: &Root) -> i64 {
    let runs = expect_ok(
        interpreter.send(holder, Symbol::intern("runs"), &[]),
        "runs",
    );
    interpreter.enter(|mutator| match mutator.fetch(&runs) {
        Value::SmallInteger(count) => count,
        _ => panic!("runs is an integer"),
    })
}

/// Which of its three states `future` is in, as a word, since a
/// `FutureState` carries handles that cannot leave the entry.
fn state_of(interpreter: &mut Interpreter, future: &Root) -> &'static str {
    interpreter.enter(|mutator| {
        let Value::Object(future) = mutator.fetch(future) else {
            panic!("a future is an object");
        };
        match mutator.future_state(future) {
            FutureState::Unresolved(_) => "unresolved",
            FutureState::Resolving => "resolving",
            FutureState::Resolved(_) => "resolved",
        }
    })
}

ns_runtime::for_each_lowering! {
    fn a_message_to_a_future_is_answered_by_its_value_computed_once(lowering) {
        let (mut interpreter, holder) = a_holder(lowering);
        let future = a_future(&mut interpreter, &holder, "three");
        let four = interpreter.stash(Value::SmallInteger(4));
        let sum = expect_ok(
            interpreter.send(&future, Symbol::intern("+"), std::slice::from_ref(&four)),
            "future + 4",
        );
        let again = expect_ok(
            interpreter.send(&future, Symbol::intern("+"), &[four]),
            "future + 4 again",
        );
        interpreter.enter(|mutator| {
            assert!(matches!(mutator.fetch(&sum), Value::SmallInteger(7)));
            assert!(matches!(mutator.fetch(&again), Value::SmallInteger(7)));
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
        let three = interpreter.stash(Value::SmallInteger(3));
        let identical = expect_ok(
            interpreter.send(&future, Symbol::intern("=="), &[three]),
            "future == 3",
        );
        let is_nil = expect_ok(
            interpreter.send(&future, Symbol::intern("isNil"), &[]),
            "future isNil",
        );
        interpreter.enter(|mutator| {
            let Value::Object(class) = mutator.fetch(&class) else {
                panic!("a class")
            };
            assert!(class.same(mutator.builtin_class(ns_runtime::BuiltinClass::Integer)));
            assert_eq!(
                mutator.heap().describe(&mutator.fetch(&printed)),
                "the String \"3\""
            );
            assert!(matches!(mutator.fetch(&identical), Value::Boolean(true)));
            assert!(matches!(mutator.fetch(&is_nil), Value::Boolean(false)));
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
        let one = interpreter.stash(Value::SmallInteger(1));
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
        let one = interpreter.stash(Value::SmallInteger(1));
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
        let four = interpreter.stash(Value::SmallInteger(4));
        let sum = expect_ok(
            interpreter.send(&outer, Symbol::intern("+"), &[four]),
            "outer + 4",
        );
        interpreter.enter(|mutator| {
            assert!(matches!(mutator.fetch(&sum), Value::SmallInteger(7)));
            let Value::Object(outer) = mutator.fetch(&outer) else {
                panic!("a future is an object");
            };
            // Resolved to the value, never to the inner future.
            assert!(matches!(
                mutator.future_state(outer),
                FutureState::Resolved(Value::SmallInteger(3))
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
        let one = interpreter.stash(Value::SmallInteger(1));
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
        let one = interpreter.stash(Value::SmallInteger(1));
        for attempt in 0..2 {
            let _ = expect_error_message(
                interpreter.send(&future, Symbol::intern("+"), std::slice::from_ref(&one)),
                "a future of a one-argument block",
            );
            assert_eq!(state_of(&mut interpreter, &future), "unresolved", "attempt {attempt}");
        }
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
        assert!(mutator.as_future(Value::SmallInteger(3)).is_none());
        assert!(mutator.as_future(Value::Nil).is_none());
    });
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
