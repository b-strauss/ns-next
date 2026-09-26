//! The exceptions `Kernel.ns` builds over the activation leaves, where a
//! golden cannot reach: the debugger's resumption of an unhandled exception,
//! which `Interpreter::resume_unhandled_exceptions` stands in for
//! (`docs/design/core-library.md`, "Exceptions").

use indoc::indoc;
use ns_runtime::{HostError, HostResult, HostSignal, Interpreter, Lowering, Root, Symbol};

fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

fn expect_error(result: HostResult, what: &str) -> HostError {
    match result {
        Err(HostSignal::Error(error)) => error,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Ok(_) => panic!("{what}: answered, where it should have ended in an error"),
    }
}

const PROBE: &str = indoc! {"
    Newspeak3
    'Test'
    class Probe = Object new () (
        public probe = ( ^self absent )
        public outOfRange = ( ^#(1 2) at: 5 )
    ) : ()
"};

/// A `Probe` in an interpreter compiling with `lowering`.
fn a_probe(lowering: Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(PROBE).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        PROBE.into(),
        ns_runtime::SourceOrigin::File("probe.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    let probe = expect_ok(interpreter.instantiate(&class, &[]), "Probe new");
    (interpreter, probe)
}

/// Squeak's `doesNotUnderstand:`: when the default handler resumed the
/// `MessageNotUnderstood` -- the debugger, once the missing method exists --
/// the message is sent again. Here the method still does not exist, so the
/// re-send fails again, and with the hook spent that second failure ends
/// the run with the error the first would have ended it with. The hook's
/// count reaching zero is what shows the first one was resumed and the
/// message sent again, rather than the run ending at the first.
#[test]
fn an_unhandled_message_not_understood_is_resent_when_resumed() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, probe) = a_probe(lowering);
        interpreter.resume_unhandled_exceptions(1);
        let error = expect_error(
            interpreter.send(&probe, Symbol::intern("probe"), &[]),
            "probe",
        );
        assert_eq!(
            error.message, "an object does not understand #absent",
            "{lowering:?}"
        );
        assert_eq!(error.selector, Symbol::intern("absent"), "{lowering:?}");
        assert_eq!(
            interpreter.unhandled_exceptions_left_to_resume(),
            0,
            "{lowering:?}: the first doesNotUnderstand: was resumed and the message sent again"
        );
    }
}

/// Without the hook, the first `doesNotUnderstand:` ends the run: nothing
/// is sent again.
#[test]
fn an_unhandled_message_not_understood_ends_the_run() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, probe) = a_probe(lowering);
        let error = expect_error(
            interpreter.send(&probe, Symbol::intern("probe"), &[]),
            "probe",
        );
        assert_eq!(
            error.message, "an object does not understand #absent",
            "{lowering:?}"
        );
        let receiver_is_the_probe = interpreter.enter(|mutator| {
            match (mutator.fetch(&error.receiver), mutator.fetch(&probe)) {
                (ns_runtime::Value::Object(receiver), ns_runtime::Value::Object(expected)) => {
                    receiver.same(expected)
                }
                _ => false,
            }
        });
        assert!(
            receiver_is_the_probe,
            "{lowering:?}: the host receives the receiver of the send nothing understood"
        );
    }
}

/// A VM error becomes an `Error`, which is not resumable: when the
/// default handler resumes it anyway, `signal` returns into the
/// continuation the VM pushed it with, and the run ends with the error as
/// it would have.
#[test]
fn a_resumed_vm_error_still_ends_the_run() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, probe) = a_probe(lowering);
        interpreter.resume_unhandled_exceptions(1);
        let error = expect_error(
            interpreter.send(&probe, Symbol::intern("outOfRange"), &[]),
            "outOfRange",
        );
        assert_eq!(
            error.message, "index 5 out of range: valid indices are 1..2 (size 2)",
            "{lowering:?}"
        );
        assert_eq!(error.selector, Symbol::intern("at:"), "{lowering:?}");
        assert_eq!(
            interpreter.unhandled_exceptions_left_to_resume(),
            0,
            "{lowering:?}: the error reached the default handler once"
        );
    }
}

const DEEP: &str = indoc! {"
    Newspeak3
    'Test'
    class Deep usingPlatform: thePlatform = Object new (
        | private Error = thePlatform kernel Error.
          public deepest ::= 0.
          public reserveDeepest ::= 0.
          public passes ::= 0. |
    ) (
        public probe: n = ( deepest:: n. ^self probe: n + 1 )
        reserveProbe: n = ( reserveDeepest:: n. ^self reserveProbe: n + 1 )
        public descend: n to: bottom = (
            n = bottom ifTrue: [^#(1) at: 5].
            ^self descend: n + 1 to: bottom
        )
        public notUnderstood: n to: bottom = (
            n = bottom ifTrue: [^nil foo].
            ^self notUnderstood: n + 1 to: bottom
        )
        descendPassing: n to: bottom = (
            n = bottom ifTrue: [^[#(1) at: 5] on: Error do: [:e | passes:: passes + 1. e pass]].
            ^self descendPassing: n + 1 to: bottom
        )
        public passInTheReserveAt: bottom = (
            ^[[self probe: 1] on: Error do: [:e | self descendPassing: 1 to: bottom]] on: Error do: [:e | 7]
        )
        public measureTheReserve = (
            ^[[self probe: 1] on: Error do: [:e | self reserveProbe: 1]] on: Error do: [:e | 7]
        )
        public failInTheReserveAt: bottom = (
            ^[[self probe: 1] on: Error do: [:e | self descend: 1 to: bottom]] on: Error do: [:e | 7]
        )
    ) : ()
"};

/// A `Deep` in an interpreter compiling with `lowering`.
fn a_deep(lowering: Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(DEEP).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        DEEP.into(),
        ns_runtime::SourceOrigin::File("deep.ns".into()),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    let platform = interpreter.platform();
    let deep = expect_ok(
        interpreter.instantiate(&class, &[platform]),
        "Deep usingPlatform:",
    );
    (interpreter, deep)
}

/// Sends `selector` to `receiver` with the small integers `arguments`.
fn send_integers(
    interpreter: &mut Interpreter,
    receiver: &Root,
    selector: &str,
    arguments: &[i64],
) -> HostResult {
    let arguments: Vec<Root> = arguments
        .iter()
        .map(|&argument| interpreter.stash(ns_runtime::Value::SmallInteger(argument)))
        .collect();
    interpreter.send(receiver, Symbol::intern(selector), &arguments)
}

/// The small integer the slot accessor `selector` answers.
fn integer_slot(interpreter: &mut Interpreter, receiver: &Root, selector: &str) -> i64 {
    let value = expect_ok(
        interpreter.send(receiver, Symbol::intern(selector), &[]),
        selector,
    );
    interpreter.enter(|mutator| match mutator.fetch(&value) {
        ns_runtime::Value::SmallInteger(integer) => integer,
        _ => panic!("{selector} answers a small integer"),
    })
}

const INDEX_ERROR: &str = "index 5 out of range: valid indices are 1..1 (size 1)";
const DEPTH_ERROR: &str = "stack overflow: recursion too deep";

/// An error raised a few activations below the depth limit, or at it, ends
/// the run as itself when nothing handles it: its `signal`, the handler
/// search and the default action run in the reserve above the limit, so
/// none of them meets the limit and replaces the error with the limit's.
/// `descend:to:` reaches the depth `probe:` measured, level for level; one
/// level further it is the limit that fails.
#[test]
fn an_unhandled_error_just_below_the_depth_limit_ends_the_run_as_itself() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, deep) = a_deep(lowering);
        let error = expect_error(
            send_integers(&mut interpreter, &deep, "probe:", &[1]),
            "probe:",
        );
        assert_eq!(error.message, DEPTH_ERROR, "{lowering:?}");
        let deepest = integer_slot(&mut interpreter, &deep, "deepest");
        for (bottom, expected) in [
            (deepest - 3, INDEX_ERROR),
            (deepest - 1, INDEX_ERROR),
            (deepest, INDEX_ERROR),
            (deepest + 1, DEPTH_ERROR),
        ] {
            let error = expect_error(
                send_integers(&mut interpreter, &deep, "descend:to:", &[1, bottom]),
                "descend:to:",
            );
            assert_eq!(
                error.message,
                expected,
                "{lowering:?}: an error {} levels below the limit",
                deepest - bottom
            );
            if expected == INDEX_ERROR {
                assert_eq!(error.selector, Symbol::intern("at:"), "{lowering:?}");
            }
        }
    }
}

/// While the reserve is held, an error raised by a handler is signalled in
/// what is left of it. With room, it is looked for outside the handler's
/// `on:do:` and caught there. So near the reserve's end that its own
/// `signal` or handler search cannot run, the run ends with that error --
/// the one being signalled -- and not with the depth limit its signalling
/// met; a handler that itself recurses past the reserve ends the run with
/// the depth limit's error.
#[test]
fn an_error_whose_signalling_overruns_the_reserve_ends_the_run_as_itself() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, deep) = a_deep(lowering);
        let error = expect_error(
            interpreter.send(&deep, Symbol::intern("measureTheReserve"), &[]),
            "measureTheReserve",
        );
        assert_eq!(
            error.message, DEPTH_ERROR,
            "{lowering:?}: a handler recursing past the reserve"
        );
        let reserve_deepest = integer_slot(&mut interpreter, &deep, "reserveDeepest");
        let caught = expect_ok(
            send_integers(
                &mut interpreter,
                &deep,
                "failInTheReserveAt:",
                &[reserve_deepest - 500],
            ),
            "failInTheReserveAt: with room",
        );
        let caught_outside = interpreter
            .enter(|mutator| matches!(mutator.fetch(&caught), ns_runtime::Value::SmallInteger(7)));
        assert!(
            caught_outside,
            "{lowering:?}: with room, the outer handler catches the error"
        );
        for bottom in [reserve_deepest - 2, reserve_deepest] {
            let error = expect_error(
                send_integers(&mut interpreter, &deep, "failInTheReserveAt:", &[bottom]),
                "failInTheReserveAt:",
            );
            assert_eq!(
                error.message,
                INDEX_ERROR,
                "{lowering:?}: an error {} levels below the reserve's end",
                reserve_deepest - bottom
            );
            assert_eq!(error.selector, Symbol::intern("at:"), "{lowering:?}");
        }
    }
}

/// §5.5 makes a message nothing understood the VM's error, so it takes the
/// reserve as any VM error does: raised a few levels below the depth limit,
/// or at it, and unhandled, it ends the run as itself, with the selector and
/// receiver of the send nothing understood.
#[test]
fn an_unhandled_does_not_understand_just_below_the_depth_limit_ends_the_run_as_itself() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, deep) = a_deep(lowering);
        let error = expect_error(
            send_integers(&mut interpreter, &deep, "probe:", &[1]),
            "probe:",
        );
        assert_eq!(error.message, DEPTH_ERROR, "{lowering:?}");
        let deepest = integer_slot(&mut interpreter, &deep, "deepest");
        for (bottom, expected) in [
            (deepest - 3, "an object does not understand #foo"),
            (deepest - 1, "an object does not understand #foo"),
            (deepest, "an object does not understand #foo"),
            (deepest + 1, DEPTH_ERROR),
        ] {
            let error = expect_error(
                send_integers(&mut interpreter, &deep, "notUnderstood:to:", &[1, bottom]),
                "notUnderstood:to:",
            );
            assert_eq!(
                error.message,
                expected,
                "{lowering:?}: a doesNotUnderstand: {} levels below the limit",
                deepest - bottom
            );
            if expected != DEPTH_ERROR {
                assert_eq!(error.selector, Symbol::intern("foo"), "{lowering:?}");
            }
        }
    }
}

/// The search a `pass` starts is the kernel's, as the first one is: an
/// overrun of the reserve during it ends the run with the error being
/// passed, not the depth limit. Near the reserve's end the passing
/// handler runs (`passes` counts it) and its `pass` cannot finish; at some
/// such depth the run must end with the index error the handler passed.
#[test]
fn a_pass_that_overruns_the_reserve_ends_the_run_with_the_passed_error() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interpreter, deep) = a_deep(lowering);
        let error = expect_error(
            interpreter.send(&deep, Symbol::intern("measureTheReserve"), &[]),
            "measureTheReserve",
        );
        assert_eq!(error.message, DEPTH_ERROR, "{lowering:?}");
        let reserve_deepest = integer_slot(&mut interpreter, &deep, "reserveDeepest");
        // From the reserve's end outwards, stopping at the first depth that
        // shows it: nearer the end the handler never runs, further out the
        // `pass` finds the outer handler.
        let overran_while_passing = (0..60).any(|offset| {
            let passes_before = integer_slot(&mut interpreter, &deep, "passes");
            let result = send_integers(
                &mut interpreter,
                &deep,
                "passInTheReserveAt:",
                &[reserve_deepest - offset],
            );
            let passed = integer_slot(&mut interpreter, &deep, "passes") > passes_before;
            matches!(result, Err(HostSignal::Error(error)) if passed && error.message == INDEX_ERROR)
        });
        assert!(
            overran_while_passing,
            "{lowering:?}: some depth ends the run with the passed error once the handler ran"
        );
    }
}
