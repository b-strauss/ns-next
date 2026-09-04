//! Native ("primitive") method dispatch for the Milestone 1 core library
//! classes. `BuiltinClass::Object` (Task 7) is the first real arm; the rest
//! land in Tasks 8-10.

pub mod object_class;

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
        // BuiltinClass::Boolean | True | False -> Task 8
        // BuiltinClass::Integer | Float -> Task 9 (numbers)
        // BuiltinClass::Str | Sym | Character -> Task 9 (text)
        // BuiltinClass::Array | OrderedCollection -> Task 10
        // BuiltinClass::Block -> Task 10
        _ => None,
    }
}
