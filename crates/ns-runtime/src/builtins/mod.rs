//! Native ("primitive") method dispatch for the Milestone 1 core library
//! classes. `BuiltinClass::Object` (Task 7) is the first real arm; the rest
//! land in Tasks 8-10.

pub mod boolean;
pub mod numbers;
pub mod object_class;
pub mod text;

use crate::types::*;
use crate::{Interpreter, Symbol};

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
        // BuiltinClass::Array | OrderedCollection -> Task 10
        // BuiltinClass::Block -> Task 10
        _ => None,
    }
}
