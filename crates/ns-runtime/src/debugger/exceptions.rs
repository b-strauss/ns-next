//! Exception breakpoints (`docs/design/debugger.md`, "Exceptions"). The
//! handler search of `Kernel.ns`'s `Exception` reports its outcome
//! through two mirror leaves: no handler, just before it sends
//! `defaultAction` (`unhandled`), and the handler it found, just before
//! that runs (`caught`). Each only decides and records: it runs no
//! Newspeak code and collects nothing, and the class an exception
//! breakpoint names is matched by walking the exception's class chain by
//! declaration. A stop it decides is recorded on the `Mutator` and asked
//! of the next poll through the request bit (`DEBUGGER_STOP`), so that the
//! program stops at the safepoint -- the push of `defaultAction`, of the
//! handler's `cull:`, or of the `resume:` that resumes a `Halt` -- before
//! that frame runs an instruction. At the stop, the stack trace shows
//! first the frame below the exception's own machinery (`first_shown`).

use std::rc::Rc;
use std::sync::PoisonError;

use super::metadata::again::{declaration_path, Member};
use super::remote::{ClassName, StopReason};
use super::stack::{self, StackFrame};
use super::stepping;
use crate::heap::{FrameReference, DEBUGGER_STOP};
use crate::interpreter::core_library::KERNEL_MODULE;
use crate::interpreter::Mutator;
use crate::mixin::Mixin;
use crate::types::BuiltinClass;
use crate::{Symbol, Value};

/// The exception breakpoints (`Command::SetExceptionBreakpoints`): an
/// uncaught exception's, a caught one's, each for the classes `classes`
/// names or, empty, for every class, and the `Halt` breakpoint. The
/// default, until the agent sets them, stops an uncaught exception and a
/// `Halt`, and no caught one.
#[derive(Clone)]
pub(crate) struct ExceptionBreakpoints {
    pub(crate) uncaught: bool,
    pub(crate) caught: bool,
    pub(crate) halt: bool,
    pub(crate) classes: Vec<ClassName>,
}

impl Default for ExceptionBreakpoints {
    fn default() -> ExceptionBreakpoints {
        ExceptionBreakpoints {
            uncaught: true,
            caught: false,
            halt: true,
            classes: Vec::new(),
        }
    }
}

/// The search for `exception`'s handler found none, and `defaultAction`
/// is about to be sent: with a debugger attached, the stop the exception
/// breakpoints ask for is recorded for the next poll. Answers whether that
/// stop is a `Halt`'s, which the kernel resumes with nil once the program
/// goes on, instead of running its default action. The test hook that
/// resumes unhandled exceptions (`Interpreter::resume_unhandled_exceptions`)
/// is served first: the exception it resumes stops nothing.
pub(crate) fn unhandled<'gc>(mutator: &mut Mutator<'gc>, exception: Value<'gc>) -> bool {
    if !mutator.breakpoints.attached() || mutator.unhandled_exceptions_to_resume > 0 {
        return false;
    }
    let breakpoints = mutator.breakpoints.exception_breakpoints().clone();
    if breakpoints.halt && is_a(mutator, exception, BuiltinClass::Halt) {
        request(mutator, StopReason::Halt);
        return true;
    }
    if breakpoints.uncaught && selects(mutator, &breakpoints.classes, exception) {
        let reason = reason(mutator, exception, false);
        request(mutator, reason);
    }
    false
}

/// The search found the handler of the `on:do:` activation `handler` for
/// `exception`, which is about to run: with a debugger attached, a step in
/// progress hears of it (`stepping::handler_found`), and the stop the
/// caught-exception breakpoint asks for is recorded for the next poll.
pub(crate) fn caught<'gc>(
    mutator: &mut Mutator<'gc>,
    exception: Value<'gc>,
    handler: FrameReference<'gc>,
) {
    if !mutator.breakpoints.attached() {
        return;
    }
    stepping::handler_found(mutator, handler);
    let breakpoints = mutator.breakpoints.exception_breakpoints().clone();
    if breakpoints.caught && selects(mutator, &breakpoints.classes, exception) {
        let reason = reason(mutator, exception, true);
        request(mutator, reason);
    }
}

/// Drops a stop an exception breakpoint asked for that no poll served:
/// the run it was asked in ended first, and the request bit with it.
pub(crate) fn drop_pending(mutator: &mut Mutator<'_>) {
    mutator.breakpoints.take_pending_exception();
    if mutator.heap.debugger_bits() & DEBUGGER_STOP == 0 {
        return;
    }
    let arming = mutator.breakpoints.arming();
    let _armed = arming
        .as_ref()
        .map(|arming| arming.lock().unwrap_or_else(PoisonError::into_inner));
    mutator.heap.clear_debugger_bits(DEBUGGER_STOP);
}

/// Records the stop `reason` for the next poll and asks for it under the
/// attached debugger's arming lock, as the agent arms the poll.
fn request(mutator: &mut Mutator<'_>, reason: StopReason) {
    mutator.breakpoints.request_exception_stop(reason);
    let arming = mutator.breakpoints.arming();
    let _armed = arming
        .as_ref()
        .map(|arming| arming.lock().unwrap_or_else(PoisonError::into_inner));
    mutator.heap.request_debugger_stop();
}

/// Whether `exception`'s class, or a superclass of it, is a class of the
/// kernel's declaration `kind` names.
fn is_a<'gc>(mutator: &Mutator<'gc>, exception: Value<'gc>, kind: BuiltinClass) -> bool {
    let class = mutator.builtin_class(kind);
    let Some(declaration) = mutator.heap.class_declaration(class) else {
        return false;
    };
    crate::builtins::activation::inherits_declaration(mutator, exception, &declaration)
}

/// Whether the exception breakpoints' `classes` take `exception`: every
/// exception for none, and otherwise one whose class chain holds a class
/// of a declaration one of them names, matched by its module and its path
/// from the module's root, as a step filter names a declaration.
fn selects<'gc>(mutator: &Mutator<'gc>, classes: &[ClassName], exception: Value<'gc>) -> bool {
    if classes.is_empty() {
        return true;
    }
    crate::builtins::activation::inherits_any(mutator, exception, |candidate| {
        names(mutator, classes, candidate)
    })
}

/// Whether one of `classes` names the declaration `mixin`.
fn names(mutator: &Mutator<'_>, classes: &[ClassName], mixin: &Rc<Mixin>) -> bool {
    let Some((root, path)) = declaration_path(mutator, mixin) else {
        return false;
    };
    classes.iter().any(|class| {
        class.module == root.name.as_str()
            && class.path.len() == path.len()
            && class
                .path
                .iter()
                .zip(&path)
                .all(|(named, declared)| named == declared.as_str())
    })
}

/// The stop of an exception breakpoint for `exception`: its class's name
/// and its description -- its `messageText` as text, or its class's name
/// when it has none, as `Exception>>description` answers unless a class
/// overrides it, which only running code could ask.
fn reason<'gc>(mutator: &Mutator<'gc>, exception: Value<'gc>, caught: bool) -> StopReason {
    let heap = &mutator.heap;
    let class_name = heap.class_display_name(mutator.class_of(exception));
    let text = exception
        .as_object()
        .and_then(|object| mutator.kernel_slot(object, BuiltinClass::Exception, "messageText"));
    let description = match text {
        None => class_name.clone(),
        Some(text) if text.is_nil() => class_name.clone(),
        // A Symbol is a String, so its text is read the same way.
        Some(text) => text
            .as_object()
            .and_then(|object| heap.str_value(object))
            .unwrap_or_else(|| heap.describe(text)),
    };
    StopReason::Exception {
        class_name,
        description,
        caught,
    }
}

/// The index of the frame an exception's stop shows first, the frames
/// above it hidden (`FrameDescription::hidden`): the first below the
/// exception's own machinery -- from the top, the frames down to the
/// kernel's `signal` or `pass` that runs the search, then each kernel
/// `signal`, `signal:` and `Object>>doesNotUnderstand:` below it, which
/// raised the exception -- and, from there, the first frame the step
/// filters do not filter (decision 16). A stack without that machinery
/// hides nothing, and one whose frames below it are all filtered shows
/// the first below it.
pub(crate) fn first_shown(mutator: &Mutator<'_>, frames: &[StackFrame]) -> usize {
    let kernel_method = |frame: &StackFrame| -> Option<Symbol> {
        let origin = stack::origin(&mutator.heap, frame)?;
        let Member::Method(selector) = origin.member else {
            return None;
        };
        let (root, _) = declaration_path(mutator, &origin.mixin)?;
        (root.shipped && root.name.as_str() == KERNEL_MODULE).then_some(selector)
    };
    let searching = |frame: &StackFrame| {
        kernel_method(frame)
            .is_some_and(|selector| selector == Symbol::SIGNAL || selector.as_str() == "pass")
    };
    let raising = |frame: &StackFrame| {
        kernel_method(frame).is_some_and(|selector| {
            selector == Symbol::SIGNAL
                || selector == Symbol::DOES_NOT_UNDERSTAND
                || selector.as_str() == "signal:"
        })
    };
    let Some(search) = frames.iter().position(searching) else {
        return 0;
    };
    let below = frames[search + 1..]
        .iter()
        .position(|frame| !raising(frame))
        .map_or(frames.len(), |offset| search + 1 + offset);
    let unfiltered = frames[below..]
        .iter()
        .position(|frame| !stepping::is_filtered(mutator, frame))
        .map_or(below, |offset| below + offset);
    unfiltered.min(frames.len().saturating_sub(1))
}
