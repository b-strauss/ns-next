//! How a send to the VM mirror runs (`docs/design/core-library.md`, "The VM
//! mirror is the whole Rust boundary"). One row of
//! `builtins::primitive::MIRROR` runs one leaf; this module is the one place
//! that turns a send's arguments into that leaf's receiver and arguments, and
//! a leaf's failure into the failure block's activation, whichever of the
//! three ways the send ran: sent to the mirror object (`execute_sent`), with
//! its failure block lowered inline (`send_inlined`, Task 3), or as the
//! primitive of a kernel method (`execute_method`, Task 4).

use crate::builtins::primitive::{mirror_entry, MirrorEntry, MirrorPrimitive, MIRROR};
use crate::interpreter::{Signal, Step, StepArguments};
use crate::types::*;
use crate::{Mutator, Symbol};

thread_local! {
    /// Each row's reported selector, interned once per thread -- the
    /// interner is per thread (`symbol.rs`), so a process-wide table of
    /// `Symbol`s would be wrong on every thread but the first.
    static REPORTED: Vec<Symbol> =
        MIRROR.iter().map(|entry| Symbol::intern(entry.reported)).collect();
}

/// The kernel selector `row`'s error texts name.
pub(crate) fn reported_selector(row: MirrorPrimitive) -> Symbol {
    REPORTED.with(|reported| reported[row.index()])
}

/// A send to the mirror object: the reference lowering's, and the optimized
/// lowering's wherever it could not lower the failure block inline.
/// `arguments` are the subject, the leaf's arguments, then the failure block.
pub(crate) fn execute_sent<'gc>(
    mutator: &mut Mutator<'gc>,
    row: MirrorPrimitive,
    arguments: &[Value<'gc>],
) -> Result<Step<'gc>, Signal<'gc>> {
    let entry = mirror_entry(row);
    // The lookup matched the selector, whose arity fixes the argument count
    // for any send a compiler emitted; the check is for a decoded send that
    // names the selector with the wrong count, which must fail as an error,
    // never as a slice index.
    let [subject, rest @ .., block] = arguments else {
        return Err(arity_error(entry, arguments.len()));
    };
    if rest.len() != entry.leaf_arity() {
        return Err(arity_error(entry, arguments.len()));
    }
    match run_leaf(mutator, row, *subject, rest) {
        Some(result) => result.map(|step| owning_its_arguments(step, rest)),
        None => Ok(failure_step(mutator, *block)),
    }
}

/// The leaf of `row`, on `subject` with `leaf_arguments`; `None` when it
/// does not recognize the subject's shape.
pub(crate) fn run_leaf<'gc>(
    mutator: &mut Mutator<'gc>,
    row: MirrorPrimitive,
    subject: Value<'gc>,
    leaf_arguments: &[Value<'gc>],
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let entry = mirror_entry(row);
    crate::builtins::execute(
        mutator,
        entry.primitive,
        subject,
        reported_selector(row),
        leaf_arguments,
    )
}

/// A leaf that activates a block with "the send's own arguments" means the
/// kernel send's -- the leaf's arguments -- not the mirror send's, which
/// also carry the subject and the failure block.
fn owning_its_arguments<'gc>(step: Step<'gc>, leaf_arguments: &[Value<'gc>]) -> Step<'gc> {
    match step {
        Step::Block {
            closure,
            arguments: StepArguments::Send,
            then,
        } => Step::Block {
            closure,
            arguments: StepArguments::Owned(leaf_arguments.to_vec()),
            then,
        },
        other => other,
    }
}

/// The failure block, activated with no arguments; a failure argument that
/// is not a block is its own value. Marked `ControlFlow` so that the depth
/// count treats it as it treats `ifTrue:`'s block: the optimized lowering
/// lowers this block inline and never activates it, so under the reference
/// lowering its activation is exempt exactly when that rule's condition holds
/// (`docs/design/compiler.md`, "How the limit counts").
fn failure_step<'gc>(mutator: &Mutator<'gc>, block: Value<'gc>) -> Step<'gc> {
    if let Value::Object(object) = block {
        if mutator.heap().is_closure(object) {
            return Step::Block {
                closure: object,
                arguments: StepArguments::ControlFlow,
                then: Continuation::Deliver,
            };
        }
    }
    Step::Value(block)
}

fn arity_error<'gc>(entry: &MirrorEntry, given: usize) -> Signal<'gc> {
    Signal::Error(NewspeakError {
        selector: Symbol::intern(entry.selector),
        receiver: Value::Nil,
        message: format!(
            "the VM mirror's {} takes {} arguments, got {given}",
            entry.selector,
            entry.leaf_arity() + 2
        ),
    })
}

#[cfg(test)]
mod tests {
    use crate::builtins::primitive::{mirror_primitive_named, MIRROR};
    use crate::{HostResult, HostSignal, Interpreter, Root, Symbol, Value};

    fn expect_ok(result: HostResult, what: &str) -> Root {
        match result {
            Ok(root) => root,
            Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
            Err(HostSignal::Error(error)) => {
                panic!("{what}: unexpected Newspeak error: {}", error.message)
            }
        }
    }

    fn the_mirror(interpreter: &mut Interpreter) -> Root {
        interpreter.enter(|mutator| {
            let mirror = mutator.vm_mirror();
            mutator.stash(mirror)
        })
    }

    #[test]
    fn a_mirror_send_runs_its_leaf_on_the_subject() {
        let mut interpreter = Interpreter::bootstrap();
        let mirror = the_mirror(&mut interpreter);
        let three = interpreter.stash(Value::SmallInteger(3));
        let four = interpreter.stash(Value::SmallInteger(4));
        let fallback = interpreter.stash(Value::SmallInteger(-1));
        let sum = expect_ok(
            interpreter.send(
                &mirror,
                Symbol::intern("integer:add:ifFail:"),
                &[three, four, fallback],
            ),
            "integer:add:ifFail:",
        );
        interpreter.enter(|mutator| {
            assert!(matches!(mutator.fetch(&sum), Value::SmallInteger(7)));
        });
    }

    /// A leaf that does not recognize its subject answers the failure
    /// argument's value -- here not a block, so it stands for itself, the
    /// convention `ifTrue:` already follows for a non-block argument.
    #[test]
    fn a_leaf_that_fails_on_its_subject_answers_the_failure_argument() {
        let mut interpreter = Interpreter::bootstrap();
        let mirror = the_mirror(&mut interpreter);
        let nothing = interpreter.stash(Value::Nil);
        let four = interpreter.stash(Value::SmallInteger(4));
        let fallback = interpreter.stash(Value::SmallInteger(-1));
        let answer = expect_ok(
            interpreter.send(
                &mirror,
                Symbol::intern("integer:add:ifFail:"),
                &[nothing, four, fallback],
            ),
            "integer:add:ifFail: on nil",
        );
        interpreter.enter(|mutator| {
            assert!(matches!(mutator.fetch(&answer), Value::SmallInteger(-1)));
        });
    }

    /// The error a leaf raises names the kernel selector, not the mirror's:
    /// the text a program sees must not change when `+` moves behind the
    /// mirror.
    #[test]
    fn an_error_a_leaf_raises_names_the_reported_selector() {
        let mut interpreter = Interpreter::bootstrap();
        let mirror = the_mirror(&mut interpreter);
        let three = interpreter.stash(Value::SmallInteger(3));
        let text = interpreter.enter(|mutator| {
            let string = mutator.new_string("x".to_string());
            mutator.stash(Value::Object(string))
        });
        let fallback = interpreter.stash(Value::Nil);
        match interpreter.send(
            &mirror,
            Symbol::intern("integer:add:ifFail:"),
            &[three, text, fallback],
        ) {
            Err(HostSignal::Error(error)) => {
                assert_eq!(error.selector, Symbol::intern("+"));
                assert!(
                    error.message.starts_with("+ requires a numeric argument"),
                    "{}",
                    error.message
                );
            }
            _ => panic!("3 + 'x' through the mirror must raise the mismatch error"),
        }
    }

    #[test]
    fn every_mirror_selector_is_unique_ends_in_if_fail_and_takes_two_more_arguments() {
        let mut seen = std::collections::HashSet::new();
        for (row, entry) in MIRROR.iter().enumerate() {
            assert!(
                seen.insert(entry.selector),
                "{} listed twice",
                entry.selector
            );
            assert!(entry.selector.ends_with("ifFail:"), "{}", entry.selector);
            let mirror_arity = entry.selector.matches(':').count();
            assert_eq!(
                mirror_arity,
                entry.leaf_arity() + 2,
                "{} must take the subject, the leaf's arguments and the failure block",
                entry.selector
            );
            assert!(
                !matches!(
                    entry.primitive,
                    crate::builtins::primitive::Primitive::Mirror(_)
                ),
                "{} runs a mirror row",
                entry.selector
            );
            let named = mirror_primitive_named(entry.selector).map(|found| found.index());
            assert_eq!(named, Some(row), "{}", entry.selector);
        }
    }

    /// `Mutator::is_vm_mirror` recognizes the one mirror instance and nothing
    /// else, including another instance of the same class -- which cannot
    /// arise through a program (the mirror's class is refused as a factory,
    /// `the_mirror_class_cannot_be_instantiated` below) but must still not
    /// be confused with the real mirror by identity alone.
    #[test]
    fn is_vm_mirror_recognizes_only_the_one_mirror_instance() {
        let mut interpreter = Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            let mirror = mutator.vm_mirror();
            assert!(mutator.is_vm_mirror(mirror));
            assert!(!mutator.is_vm_mirror(Value::SmallInteger(3)));
            assert!(!mutator.is_vm_mirror(Value::Nil));
            let platform = mutator.platform_value();
            assert!(!mutator.is_vm_mirror(platform));
        });
    }

    #[test]
    fn the_mirror_class_cannot_be_instantiated() {
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.enter(|mutator| {
            let class = mutator.builtin_class(crate::BuiltinClass::VmMirror);
            mutator.stash(Value::Object(class))
        });
        assert!(matches!(
            interpreter.send(&class, Symbol::NEW, &[]),
            Err(HostSignal::Error(_))
        ));
    }

    #[test]
    fn a_snapshot_that_reaches_the_mirror_is_refused() {
        let source = indoc::indoc! {"
            Newspeak3
            'T'
            class Holder = Object new (
                | public held |
            ) (
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = crate::resolve(
            module,
            std::rc::Rc::from(source),
            crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "Holder new");
        let mirror = the_mirror(&mut interpreter);
        let _ = expect_ok(
            interpreter.send(&instance, Symbol::intern("held:"), &[mirror]),
            "held:",
        );
        let refused = interpreter.enter(|mutator| {
            let Value::Object(class) = mutator.fetch(&class) else {
                panic!("the root is a class object");
            };
            let platform = mutator.platform_value();
            let holder = mutator.fetch(&instance);
            crate::image::snapshot(
                mutator,
                None,
                &crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
                class,
                &platform,
                &[holder],
            )
        });
        match refused {
            Err(message) => assert!(message.contains("VM mirror"), "{message}"),
            Ok(_) => panic!("an image must never carry the VM mirror"),
        }
    }
}
