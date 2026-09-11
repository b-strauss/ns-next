//! Native ("primitive") method dispatch for the Milestone 1 core library
//! classes: one module per `BuiltinClass` family -- `object_class` for
//! `Object` itself, then `boolean`, `numbers`, `text`, `collections` and

//! `block`.
pub mod block;
pub mod boolean;
pub mod collections;
pub mod numbers;
pub mod object_class;
pub mod text;

use crate::interpreter::{EvalResult, Signal};
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
/// Five functions ever invoke a block (`call_block`, below), directly or
/// transitively, and so are the ones whose own signature must carry
/// `Signal` rather than just `NewspeakError`: `boolean::dispatch` (`ifTrue:`/
/// `ifFalse:`/`and:`/`or:`/etc.), `collections::dispatch_common` (the `do:`
/// arm, shared by both `Array` and `OrderedCollection`),
/// `collections::dispatch_array` and `collections::dispatch_ordered_collection`
/// (both fall through to `dispatch_common` for any selector they don't
/// handle themselves, so `do:` reaches them too -- their own signatures must
/// match `dispatch_common`'s even though neither has a directly-block-
/// invoking arm of its own), and `block::dispatch` (`value`/`value:`/...).
/// The rest (`object_class`, `numbers`, `text::*`, `platform`) never invoke
/// a block and keep their original `Result<Value, NewspeakError>` signature,
/// wrapped into `EvalResult` right here at the single point where every arm
/// of this function's own `match` must agree on one type.
pub fn dispatch_native(
    interp: &mut Interpreter,
    kind: BuiltinClass,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<EvalResult> {
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
        BuiltinClass::Array => collections::dispatch_array(interp, receiver, selector, args),
        BuiltinClass::OrderedCollection => {
            collections::dispatch_ordered_collection(interp, receiver, selector, args)
        }
        BuiltinClass::Block => block::dispatch(interp, receiver, selector, args),
        BuiltinClass::Platform => {
            platform::dispatch(interp, receiver, selector, args).map(wrap_error)
        }
        _ => None,
    }
}

/// Lifts a plain-`NewspeakError`-returning native dispatch fn's result into
/// `EvalResult`, for the (majority of) `BuiltinClass` arms above that never
/// invoke a block and so can never actually produce a `Signal::Return`.
fn wrap_error(result: Result<Value, NewspeakError>) -> EvalResult {
    result.map_err(Signal::Error)
}

/// Invokes a block-shaped `Value` with `args` -- the single shared
/// closure-invocation entry point used by control-flow primitives
/// (`boolean.rs`'s `ifTrue:`/`and:`/etc.), collection traversal
/// (`collections.rs`'s `do:`), and `Block value`/`value:`/... itself
/// (`block.rs`). Goes straight through `Interpreter::invoke_closure`
/// rather than a full `send`, since block invocation is a Milestone 1
/// *native* primitive, not a user-overridable method, and must not depend
/// on `Block`'s own native methods being registered yet in dispatch order.
///
/// A non-closure `Value` (e.g. a control-flow primitive receiving a plain
/// value instead of a `[...]` literal) is returned as-is, matching the
/// reference library's convention that `ifTrue: aValue` degrades to
/// `aValue` when `aValue` isn't a block.
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
