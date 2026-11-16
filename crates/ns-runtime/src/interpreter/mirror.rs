//! How a send to the VM mirror runs (`docs/design/core-library.md`, "The VM
//! mirror is the whole Rust boundary"). One row of
//! `builtins::primitive::MIRROR` runs one leaf; this module is the one place
//! that turns a send's arguments into that leaf's receiver and arguments, and
//! a leaf's failure into the failure block's activation, whichever of the
//! three ways the send ran: sent to the mirror object (`execute_sent`), with
//! its failure block lowered inline (`send_inlined`,
//! `docs/design/compiler.md`, "Mirror sends"), or as the primitive of a
//! kernel method (`execute_method`, `docs/design/dispatch.md`, "Primitive
//! methods").

use super::Arguments;
use crate::builtins::primitive::{MIRROR, MirrorEntry, MirrorPrimitive, mirror_entry};
use crate::heap::Heap;
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
    arguments: Arguments<'_, 'gc>,
) -> Result<Step<'gc>, Signal<'gc>> {
    let entry = mirror_entry(row);
    // The lookup matched the selector, whose arity fixes the argument count
    // for any send a compiler emitted; the check is for a decoded send that
    // names the selector with the wrong count, which must fail as an error,
    // never as a slice index.
    if arguments.len() < 2 || arguments.len() - 2 != entry.leaf_arity() {
        return Err(arity_error(entry, arguments.len()));
    }
    let subject = arguments.get(&mutator.heap, 0);
    let block = arguments.get(&mutator.heap, arguments.len() - 1);
    let mut view: &[Value<'gc>] = &[];
    let rest = arguments.slice(1..arguments.len() - 1, &mut view);
    let answer = match run_leaf(mutator, row, subject, rest) {
        Some(Ok(step)) => return Ok(owning_its_arguments(&mutator.heap, step, rest)),
        failed => crate::interpreter::future::after_a_failed_leaf(
            mutator,
            entry.primitive,
            subject,
            reported_selector(row),
            rest,
            failed,
        ),
    };
    match answer {
        Some(result) => result.map(|step| owning_its_arguments(&mutator.heap, step, rest)),
        None => Ok(failure_step(mutator, block)),
    }
}

/// `Instruction::SendMirror`: the leaf of `primitive`, when `mirror` is the
/// VM mirror; `None` when the leaf does not recognize its subject, which
/// the VM answers by jumping to the inlined failure block.
///
/// `leaf_arguments` are the send's own arguments here -- the instruction
/// passes neither the subject nor the block as one -- so a leaf that
/// activates a block "with the send's arguments" is right as it stands.
pub(crate) fn send_inlined<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: MirrorPrimitive,
    mirror: Value<'gc>,
    subject: Value<'gc>,
    leaf_arguments: Arguments<'_, 'gc>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    if !mutator.is_vm_mirror(mirror) {
        return Some(Err(not_the_mirror(primitive, mirror)));
    }
    match run_leaf(mutator, primitive, subject, leaf_arguments) {
        Some(Ok(step)) => Some(Ok(step)),
        failed if mutator.futures_made => {
            see_through_a_failed_row(mutator, primitive, subject, leaf_arguments, failed)
        }
        failed => failed,
    }
}

/// `send_inlined`'s failure path once a future exists: out of line, with
/// the row's leaf and reported selector looked up here, so that the
/// `SendMirror` arm `vm::run` inlines carries nothing of it.
#[cold]
#[inline(never)]
fn see_through_a_failed_row<'gc>(
    mutator: &mut Mutator<'gc>,
    row: MirrorPrimitive,
    subject: Value<'gc>,
    leaf_arguments: Arguments<'_, 'gc>,
    failed: Option<Result<Step<'gc>, Signal<'gc>>>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    crate::interpreter::future::after_a_failed_leaf(
        mutator,
        mirror_entry(row).primitive,
        subject,
        reported_selector(row),
        leaf_arguments,
        failed,
    )
}

/// Only shipped core-library source -- `Kernel.ns` and
/// `PlatformFactory.ns` -- contains a `SendMirror`, and only as a send to its
/// root's `vmMirror`; the bootstrap checks that each root instance's
/// `vmMirror` holds the mirror (`interpreter::kernel`,
/// `interpreter::core_library`). No program reaches a shipped module's root
/// instance or its root class -- the platform it is handed is an instance of
/// `PlatformFactory`'s nested `Platform`, and nothing it can name answers
/// either root -- so no program can make a second root instance over
/// something else, and every object whose code sends to `vmMirror` reaches
/// the one the bootstrap checked. This is unreachable while that holds. It
/// is an error rather than a panic so that it stays one if it ever does not.
fn not_the_mirror<'gc>(primitive: MirrorPrimitive, receiver: Value<'gc>) -> Signal<'gc> {
    Signal::Error(NewspeakError::error(
        Symbol::intern(mirror_entry(primitive).selector),
        receiver,
        "internal: a mirror send's receiver is not the VM mirror",
    ))
}

/// The leaf of `row`, on `subject` with `leaf_arguments`; `None` when it
/// does not recognize the subject's shape.
pub(crate) fn run_leaf<'gc>(
    mutator: &mut Mutator<'gc>,
    row: MirrorPrimitive,
    subject: Value<'gc>,
    leaf_arguments: Arguments<'_, 'gc>,
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

/// A primitive method's leaf, on the send's own receiver and arguments --
/// the method forwards exactly those, which `primitive_method::recognize`
/// checked. `None` when the leaf declines, and the method is then activated
/// as any method is.
///
/// `selector` is the send's, which is the method's own and so the row's
/// reported selector.
#[inline(always)]
pub(crate) fn execute_method<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: MirrorPrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Result<Step<'gc>, Signal<'gc>>> {
    let leaf = mirror_entry(primitive).primitive;
    match crate::builtins::execute(mutator, leaf, receiver, selector, arguments) {
        Some(Ok(step)) => Some(Ok(step)),
        failed => crate::interpreter::future::after_a_failed_leaf(
            mutator, leaf, receiver, selector, arguments, failed,
        ),
    }
}

/// What a mirror send's leaf answers machine code
/// (`send_inlined_for_machine_code`).
pub(crate) enum MachineMirrorAnswer<'gc> {
    /// The leaf's value, the send's.
    Value(Value<'gc>),
    /// The leaf declined: the send goes on at its failure block.
    Failed,
    /// Anything else -- a receiver that is not the mirror, an error, a
    /// step, a decline once a future exists -- which the interpreter
    /// runs, the send again from the start.
    Exit,
}

/// `send_inlined`, for machine code's leaf (`jit::leaves`'s `mirror`) of a
/// leaf-safe row (`MachineCall::Leaf`): its leaf's value, its decline
/// while no future exists, and for everything else `Exit`, before
/// anything changed -- the row's leaf declines or raises before it changes
/// anything, and a decline once a program has made a future is seen
/// through by the interpreter (`see_through_a_failed_row`), never here.
pub(crate) fn send_inlined_for_machine_code<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: MirrorPrimitive,
    mirror: Value<'gc>,
    subject: Value<'gc>,
    leaf_arguments: Arguments<'_, 'gc>,
) -> MachineMirrorAnswer<'gc> {
    if !mutator.is_vm_mirror(mirror) {
        return MachineMirrorAnswer::Exit;
    }
    match run_leaf(mutator, primitive, subject, leaf_arguments) {
        Some(Ok(Step::Value(value))) => MachineMirrorAnswer::Value(value),
        None if !mutator.futures_made => MachineMirrorAnswer::Failed,
        _ => MachineMirrorAnswer::Exit,
    }
}

/// `execute_method`, for machine code's leaf stub: the leaf's value, and
/// `None` for anything else -- a decline, whose activation of the method
/// and whose futures seen through are the interpreter's, an error, a
/// step.
pub(crate) fn execute_method_for_machine_code<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: MirrorPrimitive,
    receiver: Value<'gc>,
    selector: Symbol,
    arguments: Arguments<'_, 'gc>,
) -> Option<Value<'gc>> {
    let leaf = mirror_entry(primitive).primitive;
    match crate::builtins::execute(mutator, leaf, receiver, selector, arguments) {
        Some(Ok(Step::Value(value))) => Some(value),
        _ => None,
    }
}

/// `execute_sent`, for machine code's leaf stub: the leaf's value, and
/// `None` for anything else -- a wrong argument count's error, a decline,
/// whose failure block and futures seen through are the interpreter's, an
/// error, a step.
pub(crate) fn execute_sent_for_machine_code<'gc>(
    mutator: &mut Mutator<'gc>,
    row: MirrorPrimitive,
    arguments: Arguments<'_, 'gc>,
) -> Option<Value<'gc>> {
    let entry = mirror_entry(row);
    if arguments.len() < 2 || arguments.len() - 2 != entry.leaf_arity() {
        return None;
    }
    let subject = arguments.get(&mutator.heap, 0);
    let mut view: &[Value<'gc>] = &[];
    let rest = arguments.slice(1..arguments.len() - 1, &mut view);
    match run_leaf(mutator, row, subject, rest) {
        Some(Ok(Step::Value(value))) => Some(value),
        _ => None,
    }
}

/// A leaf that activates a block with "the send's own arguments" means the
/// kernel send's -- the leaf's arguments -- not the mirror send's, which
/// also carry the subject and the failure block.
fn owning_its_arguments<'gc>(
    heap: &Heap<'gc>,
    step: Step<'gc>,
    leaf_arguments: Arguments<'_, 'gc>,
) -> Step<'gc> {
    match step {
        Step::Block {
            closure,
            arguments: StepArguments::Send,
            then,
        } => Step::Block {
            closure,
            arguments: StepArguments::Owned(leaf_arguments.to_vec(heap)),
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
    if let Some(object) = block.as_object() {
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
    Signal::Error(NewspeakError::error(
        Symbol::intern(entry.selector),
        Value::nil(),
        format!(
            "the VM mirror's {} takes {} arguments, got {given}",
            entry.selector,
            entry.leaf_arity() + 2
        ),
    ))
}

#[cfg(test)]
mod tests {
    use crate::builtins::primitive::{MIRROR, mirror_primitive_named};
    use crate::interpreter::Arguments;
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
        let three = interpreter.stash(Value::small_integer(3));
        let four = interpreter.stash(Value::small_integer(4));
        let fallback = interpreter.stash(Value::small_integer(-1));
        let sum = expect_ok(
            interpreter.send(
                &mirror,
                Symbol::intern("integer:add:ifFail:"),
                &[three, four, fallback],
            ),
            "integer:add:ifFail:",
        );
        interpreter.enter(|mutator| {
            assert_eq!(mutator.fetch(&sum).as_small_integer(), Some(7));
        });
    }

    /// A leaf that does not recognize its subject answers the failure
    /// argument's value -- here not a block, so it stands for itself, the
    /// convention `ifTrue:` already follows for a non-block argument.
    #[test]
    fn a_leaf_that_fails_on_its_subject_answers_the_failure_argument() {
        let mut interpreter = Interpreter::bootstrap();
        let mirror = the_mirror(&mut interpreter);
        let nothing = interpreter.stash(Value::nil());
        let four = interpreter.stash(Value::small_integer(4));
        let fallback = interpreter.stash(Value::small_integer(-1));
        let answer = expect_ok(
            interpreter.send(
                &mirror,
                Symbol::intern("integer:add:ifFail:"),
                &[nothing, four, fallback],
            ),
            "integer:add:ifFail: on nil",
        );
        interpreter.enter(|mutator| {
            assert_eq!(mutator.fetch(&answer).as_small_integer(), Some(-1));
        });
    }

    /// The error a leaf raises names the kernel selector, not the mirror's:
    /// the text a program sees must not change when `+` moves behind the
    /// mirror.
    #[test]
    fn an_error_a_leaf_raises_names_the_reported_selector() {
        let mut interpreter = Interpreter::bootstrap();
        let mirror = the_mirror(&mut interpreter);
        let three = interpreter.stash(Value::small_integer(3));
        let text = interpreter.enter(|mutator| {
            let string = mutator.new_string("x".to_string());
            mutator.stash(Value::object(string))
        });
        let fallback = interpreter.stash(Value::nil());
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
            assert!(!mutator.is_vm_mirror(Value::small_integer(3)));
            assert!(!mutator.is_vm_mirror(Value::nil()));
            let platform = mutator.platform_value();
            assert!(!mutator.is_vm_mirror(platform));
        });
    }

    #[test]
    fn the_mirror_class_cannot_be_instantiated() {
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.enter(|mutator| {
            let class = mutator.builtin_class(crate::BuiltinClass::VmMirror);
            mutator.stash(Value::object(class))
        });
        assert!(matches!(
            interpreter.send(&class, Symbol::NEW, &[]),
            Err(HostSignal::Error(_))
        ));
    }

    /// The bootstrap instantiates a shipped module through the ordinary
    /// `Interpreter::instantiate`, so every refusal a program's module meets
    /// reaches a shipped one too: a shipped root that is a value with a
    /// mutable slot is refused, and the bootstrap panics on it, rather than
    /// booting with a mutable value.
    #[test]
    fn a_defective_shipped_root_is_refused_to_the_bootstrap() {
        let source = indoc::indoc! {"
            class K vmMirror: theMirror = Object <: Value (
              | private vmMirror = theMirror. public count ::= 0. |
            ) () : ()
        "};
        let module = ns_syntax::parse(source).expect("parse");
        let origin = crate::SourceOrigin::File(std::path::PathBuf::from("t.ns"));
        let Ok(resolved) =
            crate::resolver::resolve_kernel(module, std::rc::Rc::from(source), origin)
        else {
            panic!("the defective module does not resolve under `resolve_kernel`");
        };
        let mut interpreter = Interpreter::bootstrap();
        let class = interpreter.load_module(&resolved);
        let mirror = the_mirror(&mut interpreter);
        match interpreter.instantiate(&class, &[mirror]) {
            Ok(_) => panic!("the bootstrap instantiated a shipped root with a mutable value slot"),
            Err(HostSignal::Return) => panic!("a non-local return escaped"),
            Err(HostSignal::Error(error)) => assert_eq!(
                error.message,
                "cannot instantiate K: a value object is deeply immutable, and it declares \
                 the mutable slot count"
            ),
        }
    }

    /// A kernel-shaped module, resolved by `resolve_kernel` when `kernel`
    /// and by plain `resolve` otherwise: its `Probe` sends the mirror inline
    /// (`sizeOf:`) and declares two primitive methods, one whose leaf accepts
    /// any receiver (`same:`) and one whose leaf never accepts a `Probe`
    /// (`size`). Answers the interpreter and a `Probe`.
    fn a_probe(lowering: crate::Lowering, kernel: bool) -> (Interpreter, Root) {
        let source = indoc::indoc! {"
            class K vmMirror: theMirror = Object new (
              | private vmMirror = theMirror. |
            ) (
              class Probe = Object new () (
                public sizeOf: text = ( ^vmMirror stringSize: text ifFail: [^-1] )
                public same: other = ( ^vmMirror object: self identicalTo: other ifFail: [^-1] )
                public size = ( ^vmMirror stringSize: self ifFail: [^-1] )
              )
              public probe = ( ^Probe new )
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("parse");
        let origin = crate::SourceOrigin::File(std::path::PathBuf::from("t.ns"));
        let resolved = if kernel {
            crate::resolver::resolve_kernel(module, std::rc::Rc::from(source), origin)
        } else {
            crate::resolve(module, std::rc::Rc::from(source), origin)
        };
        let Ok(resolved) = resolved else {
            let resolver = if kernel { "resolve_kernel" } else { "resolve" };
            panic!("the kernel-shaped module does not resolve under `{resolver}`");
        };
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let class = interpreter.load_module(&resolved);
        let mirror = the_mirror(&mut interpreter);
        let instance = expect_ok(
            interpreter.instantiate(&class, &[mirror]),
            "K vmMirror: the mirror",
        );
        let probe = expect_ok(
            interpreter.send(&instance, Symbol::intern("probe"), &[]),
            "probe",
        );
        (interpreter, probe)
    }

    /// `selector`'s compiled method on the class of the value `probe` holds.
    fn probe_method(
        interpreter: &mut Interpreter,
        probe: &Root,
        selector: &str,
    ) -> std::rc::Rc<crate::mixin::MixinMethod> {
        interpreter.enter(|mutator| {
            let receiver = mutator.fetch(probe);
            let class = mutator.class_of(receiver);
            let Some(application) = mutator.heap().class_application(class) else {
                panic!("a Probe's class is a class");
            };
            let Some(method) = application.mixin.compiled_method(Symbol::intern(selector)) else {
                panic!("Probe declares {selector}");
            };
            method.clone()
        })
    }

    crate::for_each_lowering! {
        /// A kernel-shaped module's mirror send, run end to end: under the
        /// optimized lowering its failure block is inlined (`SendMirror`), under
        /// the reference lowering it is a real closure sent to the mirror
        /// (`SendInlinable`), and both answer the leaf's value when the leaf
        /// recognizes its subject and the block's `^` when it does not.
        fn an_inlined_mirror_send_runs_its_leaf_and_its_failure_block(lowering) {
            let (mut interpreter, probe) = a_probe(lowering, true);
            let text = interpreter.enter(|mutator| {
                let string = mutator.new_string("abc".to_string());
                mutator.stash(Value::object(string))
            });
            let size = expect_ok(
                interpreter.send(&probe, Symbol::intern("sizeOf:"), &[text]),
                "sizeOf: 'abc'",
            );
            let nothing = interpreter.stash(Value::nil());
            let declined = expect_ok(
                interpreter.send(&probe, Symbol::intern("sizeOf:"), &[nothing]),
                "sizeOf: nil",
            );
            interpreter.enter(|mutator| {
                assert_eq!(
                    mutator.fetch(&size).as_small_integer(),
                    Some(3),
                    "{lowering:?}: the leaf answers the string's size"
                );
                assert_eq!(
                    mutator.fetch(&declined).as_small_integer(),
                    Some(-1),
                    "{lowering:?}: the failure block answers for nil"
                );
            });
        }
    }

    crate::for_each_lowering! {
        /// A primitive method answers from its leaf when the leaf accepts the
        /// receiver, and through its activated body -- the failure block --
        /// when it does not, alike under both lowerings.
        fn a_primitive_method_runs_its_leaf_and_its_body_only_when_the_leaf_declines(lowering) {
            let (mut interpreter, probe) = a_probe(lowering, true);
            for selector in ["same:", "size"] {
                let method = probe_method(&mut interpreter, &probe, selector);
                assert!(
                    matches!(
                        method.fast_path(),
                        Some(crate::mixin::MethodFastPath::Mirror(_))
                    ),
                    "{lowering:?}: {selector} is a primitive method"
                );
            }
            let inline = probe_method(&mut interpreter, &probe, "sizeOf:");
            assert!(
                inline.fast_path().is_none(),
                "{lowering:?}: sizeOf: forwards a parameter as the subject, not self"
            );
            let same = expect_ok(
                interpreter.send(
                    &probe,
                    Symbol::intern("same:"),
                    std::slice::from_ref(&probe),
                ),
                "aProbe same: aProbe",
            );
            let size = expect_ok(
                interpreter.send(&probe, Symbol::intern("size"), &[]),
                "aProbe size",
            );
            interpreter.enter(|mutator| {
                assert_eq!(
                    mutator.fetch(&same).as_boolean(),
                    Some(true),
                    "{lowering:?}: the leaf answers identity"
                );
                assert_eq!(
                    mutator.fetch(&size).as_small_integer(),
                    Some(-1),
                    "{lowering:?}: the activated body's failure block answers"
                );
            });
        }
    }

    /// `act` itself answers an accepting leaf without an activation, and
    /// hands a declining one's method on to be activated.
    #[test]
    fn act_answers_an_accepting_primitive_method_without_an_activation() {
        let (mut interpreter, probe) = a_probe(crate::Lowering::Optimized, true);
        let same = probe_method(&mut interpreter, &probe, "same:");
        let size = probe_method(&mut interpreter, &probe, "size");
        interpreter.enter(|mutator| {
            let receiver = mutator.fetch(&probe);
            let class = mutator.class_of(receiver);
            let accepted = mutator.act_with_base(
                crate::interpreter::resolve::Resolved::Method {
                    method: same,
                    defining_class: class,
                    chain: crate::interpreter::resolve::Chain::Public,
                },
                crate::interpreter::inline_cache::FeedbackBase::NOT_ASSIGNED,
                receiver,
                Symbol::intern("same:"),
                Arguments::Values(&[receiver].as_slice()),
            );
            assert!(matches!(
                accepted,
                Ok(crate::interpreter::Step::Value(answer)) if answer.as_boolean() == Some(true)
            ));
            let declined = mutator.act_with_base(
                crate::interpreter::resolve::Resolved::Method {
                    method: size,
                    defining_class: class,
                    chain: crate::interpreter::resolve::Chain::Public,
                },
                crate::interpreter::inline_cache::FeedbackBase::NOT_ASSIGNED,
                receiver,
                Symbol::intern("size"),
                Arguments::NONE,
            );
            assert!(matches!(
                declined,
                Ok(crate::interpreter::Step::Method { .. })
            ));
        });
    }

    crate::for_each_lowering! {
        /// The same forwarders in a module plain `resolve` resolved are ordinary
        /// methods: only a shipped module's `vmMirror` is known to hold the
        /// mirror.
        fn forwarders_outside_the_kernel_are_ordinary_methods(lowering) {
            let (mut interpreter, probe) = a_probe(lowering, false);
            for selector in ["same:", "size"] {
                let method = probe_method(&mut interpreter, &probe, selector);
                assert!(
                    method.fast_path().is_none(),
                    "{lowering:?}: {selector} outside the kernel"
                );
            }
        }
    }

    #[test]
    fn a_snapshot_that_reaches_the_mirror_is_refused() {
        let source = indoc::indoc! {"
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
            let Some(class) = mutator.fetch(&class).as_object() else {
                panic!("the root is a class object");
            };
            let holder = mutator.fetch(&instance);
            let origin = crate::SourceOrigin::File(std::path::PathBuf::from("t.ns"));
            let modules = [crate::image::ModuleSource {
                class,
                origin: &origin,
                with_source: false,
            }];
            crate::image::snapshot(mutator, &modules, &[holder])
        });
        match refused {
            Err(message) => assert!(message.contains("VM mirror"), "{message}"),
            Ok(_) => panic!("an image must never carry the VM mirror"),
        }
    }
}
