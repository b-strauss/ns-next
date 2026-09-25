//! The exceptions `Kernel.ns` builds over the activation leaves, where a
//! golden cannot reach: the debugger's resumption of an unhandled exception,
//! which `Interpreter::resume_unhandled_exceptions` stands in for
//! (`docs/design/core-library.md`, "Planned: exceptions").

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
