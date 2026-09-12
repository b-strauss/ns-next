//! Native ("primitive") method dispatch for the core library classes: one
//! module per `BuiltinClass` family -- `object_class` for `Object` itself,
//! then `boolean`, `numbers`, `text`, `collections` and `block`.
pub mod block;
pub mod boolean;
pub mod collections;
pub mod numbers;
pub mod object_class;
pub mod text;

use crate::interpreter::{EvalResult, Signal, Step};
use crate::types::*;
use crate::{platform, Interpreter, Symbol};

/// `None` = this builtin does not implement `selector` (let the caller fall
/// through to `doesNotUnderstand:`). `Some(Err(Signal::Error(_)))` = a
/// genuine Newspeak-level error (e.g. arithmetic overflow).
/// `Some(Err(Signal::Return(..)))` = a non-local return (`^expr` inside a
/// block argument, e.g. `coll do: [:each | ... ifTrue: [^x]]`) genuinely
/// escaping a native primitive's block invocation -- the
/// `blocks_and_return.ns` fixture depends on this propagating all the way
/// out to the enclosing method activation rather than being swallowed here.
///
/// `boolean::dispatch` and `block::dispatch` answer a `Step` themselves: the
/// control-flow natives and `value` return the block to activate instead of
/// calling it (`docs/design/execution.md`, "The trampoline").
///
/// Only `do:` still invokes a block from Rust (`call_block`, below), so the
/// functions it is reached through are the ones whose own signature must
/// carry `Signal` rather than just `NewspeakError`:
/// `collections::dispatch_common` (the `do:` arm, shared by both `Array` and
/// `OrderedCollection`), and `collections::dispatch_array` and
/// `collections::dispatch_ordered_collection`, which fall through to
/// `dispatch_common` for any selector they don't handle themselves. The rest
/// (`object_class`, `numbers`, `text::*`, `platform`) never invoke a block
/// and keep their `Result<Value, NewspeakError>` signature. Their results are
/// wrapped into `Step::Value` right here, by `value_step`/`wrap_error`.
pub(crate) fn dispatch_native(
    interp: &mut Interpreter,
    kind: BuiltinClass,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Step, Signal>> {
    match kind {
        BuiltinClass::Object => {
            object_class::dispatch(interp, receiver, selector, args).map(wrap_error)
        }
        BuiltinClass::Boolean | BuiltinClass::True | BuiltinClass::False => {
            boolean::dispatch(interp, receiver, selector, args)
        }
        BuiltinClass::Integer => {
            numbers::dispatch_integer(interp, receiver, selector, args).map(wrap_error)
        }
        BuiltinClass::Float => {
            numbers::dispatch_float(interp, receiver, selector, args).map(wrap_error)
        }
        BuiltinClass::Str => {
            text::dispatch_string(interp, receiver, selector, args).map(wrap_error)
        }
        BuiltinClass::Sym => {
            text::dispatch_symbol(interp, receiver, selector, args).map(wrap_error)
        }
        BuiltinClass::Character => {
            text::dispatch_character(interp, receiver, selector, args).map(wrap_error)
        }
        BuiltinClass::Array => {
            collections::dispatch_array(interp, receiver, selector, args).map(value_step)
        }
        BuiltinClass::OrderedCollection => {
            collections::dispatch_ordered_collection(interp, receiver, selector, args)
                .map(value_step)
        }
        BuiltinClass::Block => block::dispatch(interp, receiver, selector, args),
        BuiltinClass::Platform => {
            platform::dispatch(interp, receiver, selector, args).map(wrap_error)
        }
        _ => None,
    }
}

/// A native's value as the step it is.
fn value_step(result: EvalResult) -> Result<Step, Signal> {
    result.map(Step::Value)
}

/// Lifts a plain-`NewspeakError`-returning native dispatch fn's result into
/// the step it is, for the (majority of) `BuiltinClass` arms above that never
/// invoke a block and so can never actually produce a `Signal::Return`.
fn wrap_error(result: Result<Value, NewspeakError>) -> Result<Step, Signal> {
    result.map(Step::Value).map_err(Signal::Error)
}

/// Invokes a block-shaped `Value` with `args`, from Rust. Its one caller is
/// `do:` (`collections.rs`): `do:` walks a collection in a Rust loop, so it
/// cannot hand the block back as a trampoline step the way the control-flow
/// natives do, and it stays a re-entry of the run loop until it moves to
/// Newspeak after the inline caches (`docs/design/core-library.md`). Goes
/// straight through `Interpreter::invoke_closure` rather than a full `send`,
/// since block invocation is a *native* primitive, not a user-overridable
/// method.
///
/// A non-closure `Value` is returned as-is, matching the reference library's
/// convention that a non-block stands for its own value.
///
/// Returns `EvalResult`, forwarding `invoke_closure`'s own result directly,
/// so a `Signal::Return` escaping the block passes straight through. It
/// must not be converted into a `NewspeakError` here: the ordinary case is
/// a return aimed at a still-live activation further up the very call stack
/// this primitive is itself running on -- `coll do: [:each | each = target
/// ifTrue: [^'found']]`, where `^'found'` targets the enclosing method, not
/// `do:`'s or `ifTrue:`'s own activation. Converting unconditionally breaks
/// every `ifTrue: [^expr]` and `do: [:e | ... ^expr]` idiom there is. A
/// return that really is aimed at a dead frame becomes a
/// `cannotReturn`-style `NewspeakError` further down, in
/// `Interpreter::raise_return`'s own check that the home is still live
/// (`interpreter/mod.rs`), and that check runs regardless of how many
/// native primitive frames the signal passes through on its way up -- so
/// nothing here needs to re-implement it.
pub(super) fn call_block(interp: &mut Interpreter, block: &Value, args: Vec<Value>) -> EvalResult {
    match block {
        Value::Obj(obj) => {
            let closure_data = interp.heap.closure_data(*obj);
            match closure_data {
                Some(closure) => interp.invoke_closure(&closure, args),
                None => Ok(*block),
            }
        }
        other => Ok(*other),
    }
}
