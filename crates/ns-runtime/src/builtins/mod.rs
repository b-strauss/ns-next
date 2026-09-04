//! Native ("primitive") method dispatch for the Milestone 1 core library
//! classes. TEMPORARY (Task 6): this is a stub that always returns `None`
//! (falls through to `doesNotUnderstand:`) so `interpreter::mod` and
//! `interpreter::eval` -- which reference `dispatch_native` -- compile and
//! this task's own tests can run. Task 7 onward flesh this out for real,
//! starting with `BuiltinClass::Object`.

use crate::interpreter::Interpreter;
use crate::types::{BuiltinClass, NewspeakError, Value};
use crate::Symbol;

pub fn dispatch_native(
    _interp: &mut Interpreter,
    _kind: BuiltinClass,
    _receiver: Value,
    _selector: Symbol,
    _args: &[Value],
) -> Option<Result<Value, NewspeakError>> {
    None
}
