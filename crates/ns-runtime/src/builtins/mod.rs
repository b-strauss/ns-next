//! Native ("primitive") method dispatch for the Milestone 1 core library
//! classes. `BuiltinClass::Object` (Task 7) is the first real arm; Tasks
//! 8-10 add the rest.

pub mod block;
pub mod boolean;
pub mod collections;
pub mod numbers;
pub mod object_class;
pub mod text;

use crate::types::*;
use crate::{platform, Interpreter, Symbol};

/// `None` = this builtin does not implement `selector` (let the caller fall
/// through to `doesNotUnderstand:`). `Some(Err(_))` = a genuine Newspeak-
/// level error (e.g. arithmetic overflow in Task 9).
pub fn dispatch_native(
    interp: &mut Interpreter,
    kind: BuiltinClass,
    receiver: Value,
    selector: Symbol,
    args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    match kind {
        BuiltinClass::Object => object_class::dispatch(interp, receiver, selector, args),
        BuiltinClass::Boolean | BuiltinClass::True | BuiltinClass::False => {
            boolean::dispatch(interp, receiver, selector, args)
        }
        BuiltinClass::Integer => numbers::dispatch_integer(interp, receiver, selector, args),
        BuiltinClass::Float => numbers::dispatch_float(interp, receiver, selector, args),
        BuiltinClass::Str => text::dispatch_string(interp, receiver, selector, args),
        BuiltinClass::Sym => text::dispatch_symbol(interp, receiver, selector, args),
        BuiltinClass::Character => text::dispatch_character(interp, receiver, selector, args),
        BuiltinClass::Array => collections::dispatch_array(interp, receiver, selector, args),
        BuiltinClass::OrderedCollection => {
            collections::dispatch_ordered_collection(interp, receiver, selector, args)
        }
        BuiltinClass::Block => block::dispatch(interp, receiver, selector, args),
        BuiltinClass::Platform => platform::dispatch(interp, receiver, selector, args),
        _ => None,
    }
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
pub(super) fn call_block(
    interp: &mut Interpreter,
    block: &Value,
    args: Vec<Value>,
) -> Result<Value, NewspeakError> {
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
                        .map_err(|signal| match signal {
                            crate::interpreter::Signal::Error(e) => e,
                            crate::interpreter::Signal::Return(..) => NewspeakError {
                                selector: Symbol::intern("value"),
                                receiver: block.clone(),
                                message:
                                    "non-local return escaped a block invoked as a native primitive"
                                        .to_string(),
                            },
                        })
                }
                None => Ok(block.clone()),
            }
        }
        other => Ok(other.clone()),
    }
}
