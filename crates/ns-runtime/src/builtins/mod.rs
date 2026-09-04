//! Native ("primitive") method dispatch for the Milestone 1 core library
//! classes. `BuiltinClass::Object` (Task 7) is the first real arm; Tasks
//! 8-10 add the rest.

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
/// genuine Newspeak-level error (e.g. arithmetic overflow in Task 9).
/// `Some(Err(Signal::Return(..)))` = a non-local return (`^expr` inside a
/// block argument, e.g. `coll do: [:each | ... ifTrue: [^x]]`) genuinely
/// escaping a native primitive's block invocation -- Task 12's own
/// `blocks_and_return.ns` fixture depends on this propagating all the way
/// out to the enclosing method activation rather than being swallowed here.
///
/// Only `boolean::dispatch`, `collections::dispatch_ordered_collection`
/// (via its shared `dispatch_common` for `do:`), and `block::dispatch` ever
/// invoke a block (`call_block`, below) and so are the only per-class
/// `dispatch` functions whose own signature must carry `Signal` rather than
/// just `NewspeakError` -- the rest (`object_class`, `numbers`, `text::*`,
/// `collections::dispatch_array`, `platform`) never invoke a block and keep
/// their original `Result<Value, NewspeakError>` signature, wrapped into
/// `EvalResult` right here at the single point where every arm's result
/// must agree on one type.
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
/// (`block.rs`). Goes straight through `interpreter::eval::invoke_closure`
/// rather than a full `send`, since block invocation is a Milestone 1
/// *native* primitive, not a user-overridable method, and must not depend
/// on `Block`'s own native methods being registered yet in dispatch order.
///
/// A non-closure `Value` (e.g. a control-flow primitive receiving a plain
/// value instead of a `[...]` literal) is returned as-is, matching the
/// reference library's convention that `ifTrue: aValue` degrades to
/// `aValue` when `aValue` isn't a block.
///
/// Originally a local `invoke_block` helper duplicated inside `boolean.rs`
/// (Task 8); moved here and generalized to take real `args` so `do:`
/// (Task 10) can reuse it instead of accumulating a third copy. Keeps
/// reading `closure.bindings.clone()` -- the resolver's bindings map the
/// block body was originally resolved under (Task 8's fix) -- rather than
/// a fresh empty map; regressing that was a real, task-reviewed bug in an
/// earlier draft.
///
/// Task 12 fix: returns `EvalResult` (forwarding `invoke_closure`'s own
/// result directly) rather than the earlier `Result<Value, NewspeakError>`,
/// which used to convert *every* `Signal::Return` escaping the block into a
/// generic "non-local return escaped a block invoked as a native primitive"
/// `NewspeakError` -- unconditionally, even when the return's target frame
/// was a still-live activation further up the very call stack this
/// primitive is itself running on (the ordinary, correct case for e.g.
/// `coll do: [:each | each = target ifTrue: [^'found']]`, where the
/// `^'found'` targets the enclosing method, not `do:`'s or `ifTrue:`'s own
/// activation). That made *every* `ifTrue: [^expr]`/`do: [:e | ... ^expr]`
/// idiom fail; a genuine non-local-return-to-a-dead-frame case is already
/// correctly turned into a `cannotReturn`-style `NewspeakError` further
/// down, by `Interpreter::raise_return`'s own `uncontinuable` check (see
/// `interpreter/mod.rs`) -- that check runs regardless of how many native
/// primitive frames the `Signal::Return` passes through on its way up, so
/// nothing here needs to re-implement it.
pub(super) fn call_block(interp: &mut Interpreter, block: &Value, args: Vec<Value>) -> EvalResult {
    match block {
        Value::Obj(obj) => {
            let closure_data = match &obj.borrow().data {
                ObjectData::Closure(c) => Some(c.clone()),
                _ => None,
            };
            match closure_data {
                Some(closure_data) => {
                    let closure = std::rc::Rc::new(closure_data);
                    let bindings = closure.bindings.clone();
                    crate::interpreter::eval::invoke_closure(interp, &closure, args, &bindings)
                }
                None => Ok(block.clone()),
            }
        }
        other => Ok(other.clone()),
    }
}
