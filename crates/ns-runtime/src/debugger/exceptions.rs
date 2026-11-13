//! Exception breakpoints (`docs/design/debugger.md`, "Exceptions"). The
//! handler search of `Kernel.ns`'s `Exception` reports its outcome
//! through two mirror rows: no handler, just before it sends
//! `defaultAction` (`unhandled`), a leaf machine code calls without
//! exiting, and the handler it found, just before that runs (`caught`),
//! reported by the exit that reads the handler for the search. Beyond the
//! handler the exit answers, each only decides and records, and only with
//! a debugger attached: it runs no Newspeak code and collects nothing, and
//! the class an exception breakpoint names is matched by walking the
//! exception's class chain by declaration. What it decides is recorded on
//! the `Mutator` and asked of the next poll through the request bit
//! (`DEBUGGER_STOP`), whose safepoint serves it (`serve`): reports a
//! handler found to a step in progress and stops once the frame the
//! search runs next -- `defaultAction`, the handler block, the `resume:`
//! that resumes a `Halt` -- stands at its first instruction. At the stop,
//! the stack trace hides the exception's own machinery (`hidden`): it
//! shows first the frame below it (`first_shown`), or, at a caught
//! exception's, the handler on top and then that frame.

use std::rc::Rc;
use std::sync::PoisonError;

use super::metadata::again::{declaration_path, Member};
use super::remote::{ClassName, StopReason};
use super::stack::{self, StackFrame};
use super::stepping;
use crate::heap::DEBUGGER_STOP;
use crate::interpreter::core_library::KERNEL_MODULE;
use crate::interpreter::Mutator;
use crate::mixin::Mixin;
use crate::types::BuiltinClass;
use crate::{Root, Symbol, Value};

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

/// What the handler search's rows recorded for the next poll's
/// safepoint (`serve`): the stop an exception breakpoint asks for, and,
/// for a handler found, its `on:do:` activation, which the safepoint
/// reports to a step in progress, and its handler, whose first
/// instruction a caught exception's stop waits for; each held as a
/// `Root`.
pub(crate) struct PendingException {
    /// The stop, `None` when only a step in progress is to hear of the
    /// handler.
    reason: Option<StopReason>,
    /// The `on:do:` activation's frame object, until it is reported.
    activation: Option<Root>,
    /// The handler, for a handler found.
    handler: Option<Root>,
}

/// The search for `exception`'s handler found none, and `defaultAction`
/// is about to be sent: with a debugger attached, the stop the exception
/// breakpoints ask for is recorded for the next poll. Answers whether that
/// stop is a `Halt`'s, which the kernel resumes with nil once the program
/// goes on, instead of running its default action. The test hook that
/// resumes unhandled exceptions (`Interpreter::resume_unhandled_exceptions`)
/// is served first: the exception it resumes stops nothing; and nothing
/// stops while the debugger's termination unwinds the run. A leaf machine
/// code calls (`MachineCall::Leaf`): it reads the exception and its class
/// chain, records and sets the request bit, and nothing more.
pub(crate) fn unhandled<'gc>(mutator: &mut Mutator<'gc>, exception: Value<'gc>) -> bool {
    if !mutator.breakpoints.attached()
        || mutator.breakpoints.terminating()
        || mutator.unhandled_exceptions_to_resume > 0
    {
        return false;
    }
    let breakpoints = mutator.breakpoints.exception_breakpoints().clone();
    if breakpoints.halt && is_a(mutator, exception, BuiltinClass::Halt) {
        request(
            mutator,
            PendingException {
                reason: Some(StopReason::Halt { exits: Vec::new() }),
                activation: None,
                handler: None,
            },
        );
        return true;
    }
    if breakpoints.uncaught && selects(mutator, &breakpoints.classes, exception) {
        let reason = reason(mutator, exception, false);
        request(
            mutator,
            PendingException {
                reason: Some(reason),
                activation: None,
                handler: None,
            },
        );
    }
    false
}

/// The search found `handler`, the handler of the `on:do:` activation
/// `activation`, for `exception`, and is about to run it: with a debugger
/// attached, what the safepoint does with it is recorded for the next
/// poll -- a step in progress hears of it there, and the stop the
/// caught-exception breakpoint asks for waits there for the handler's
/// first instruction (`serve`). Run by the mirror send that reads the
/// handler (`exceptionCaught:at:ifFail:`), which machine code exits for
/// and which reads the handler off the `on:do:` activation's frame
/// itself: only flooding a step's frame, which frees machine code, and the
/// step's reading of the handler's frame wait for the safepoint.
pub(crate) fn caught<'gc>(
    mutator: &mut Mutator<'gc>,
    exception: Value<'gc>,
    activation: Value<'gc>,
    handler: Value<'gc>,
) {
    if !mutator.breakpoints.attached() || mutator.breakpoints.terminating() {
        return;
    }
    let breakpoints = mutator.breakpoints.exception_breakpoints().clone();
    let reason = (breakpoints.caught && selects(mutator, &breakpoints.classes, exception))
        .then(|| reason(mutator, exception, true));
    if reason.is_none() && mutator.stepping.is_none() {
        return;
    }
    let pending = PendingException {
        reason,
        activation: Some(mutator.heap.stash(activation)),
        handler: Some(mutator.heap.stash(handler)),
    };
    request(mutator, pending);
}

/// What the safepoint does with what the rows recorded: a handler found
/// is reported to a step in progress (`stepping::handler_found`), once,
/// where flooding is allowed; then the stop, if any, is answered -- but a
/// caught exception's waits, recorded again with its request bit, while
/// the frame on top is the kernel's `cull:` sent to the handler, so that it
/// stops in the handler block at its first instruction, or at the first
/// frame of a handler that is no block.
pub(crate) fn serve(mutator: &mut Mutator<'_>, pending: PendingException) -> Option<StopReason> {
    let PendingException {
        reason,
        activation,
        handler,
    } = pending;
    if let Some(activation) = &activation {
        let frame = mutator
            .heap
            .fetch(activation)
            .as_object()
            .and_then(|object| mutator.heap.object_frame(object))
            .filter(|frame| mutator.heap.frame_is_running(*frame));
        if let Some(frame) = frame {
            stepping::handler_found(mutator, frame);
        }
    }
    let reason = reason?;
    if still_searching(mutator, handler.as_ref()) {
        let waiting = PendingException {
            reason: Some(reason),
            activation: None,
            handler,
        };
        request(mutator, waiting);
        return None;
    }
    Some(reason)
}

/// Whether the frame on top is still the search's, short of what the stop
/// waits for: `invokeNextHandler` itself, for an uncaught exception's stop
/// -- machine code's call of `defaultAction` or of `resume:` refused
/// under the lowered depth limit leaves it on top at the send, before the
/// push -- and for a caught one's, the kernel's block that invokes
/// `handler`, likewise, or the kernel's `Closure>>cull:` sent to it, which
/// pushes the handler block next.
fn still_searching(mutator: &Mutator<'_>, handler: Option<&Root>) -> bool {
    let heap = &mutator.heap;
    let Some(top) = heap.top_activation() else {
        return false;
    };
    let Some(selector) = kernel_method(mutator, &StackFrame::Running(top)) else {
        return false;
    };
    let Some(handler) = handler else {
        return selector.as_str() == "invokeNextHandler";
    };
    if selector.as_str() == "invokeHandlerAt:disabling:" {
        return true;
    }
    let receives = match (
        heap.activation_receiver(top).as_object(),
        heap.fetch(handler).as_object(),
    ) {
        (Some(receiver), Some(handler)) => receiver.same(handler),
        _ => false,
    };
    receives && selector.as_str() == "cull:"
}

/// Drops what the rows recorded that no poll served, and the request
/// bit with it, and ends a termination: the run ended.
pub(crate) fn drop_pending(mutator: &mut Mutator<'_>) {
    mutator.breakpoints.take_pending_exception();
    mutator.breakpoints.set_terminating(false);
    if mutator.heap.debugger_bits() & DEBUGGER_STOP == 0 {
        return;
    }
    let arming = mutator.breakpoints.arming();
    let _armed = arming
        .as_ref()
        .map(|arming| arming.lock().unwrap_or_else(PoisonError::into_inner));
    mutator.heap.clear_debugger_bits(DEBUGGER_STOP);
}

/// Records `pending` for the next poll and asks for it under the attached
/// debugger's arming lock, as the agent arms the poll.
fn request(mutator: &mut Mutator<'_>, pending: PendingException) {
    mutator.breakpoints.request_exception_stop(pending);
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
        exits: Vec::new(),
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
    let kernel_method = |frame: &StackFrame| kernel_method(mutator, frame);
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

/// The frames an exception's stop hides: at an uncaught exception's and a
/// `Halt`'s, every frame above the one `first_shown` answers; at a caught
/// one's, which stands in the handler about to run -- where the program
/// goes on -- the frames between that handler, on top and shown first,
/// and the one `first_shown` answers below the machinery, which is shown
/// next. A handler whose `cull:` a fast path answered pushed nothing, and
/// the machinery frame the stop stands in instead stays shown.
pub(crate) fn hidden(
    mutator: &Mutator<'_>,
    frames: &[StackFrame],
    caught: bool,
) -> std::ops::Range<usize> {
    let first = first_shown(mutator, frames);
    if caught {
        first.min(1)..first
    } else {
        0..first
    }
}

/// The selector of the kernel's method `frame` runs; `None` for a frame
/// that runs no method of the shipped `Kernel`.
fn kernel_method(mutator: &Mutator<'_>, frame: &StackFrame) -> Option<Symbol> {
    let origin = stack::origin(&mutator.heap, frame)?;
    let Member::Method(selector) = origin.member else {
        return None;
    };
    let (root, _) = declaration_path(mutator, &origin.mixin)?;
    (root.shipped && root.name.as_str() == KERNEL_MODULE).then_some(selector)
}
