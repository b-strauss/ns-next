//! §3.1.1's conditions on a value *class*, decided where a nested class
//! object is created (`nested::after_clause`), as SOMns decides them
//! (`InstantiationNode.signalExceptionsIfFaultFoundElseReturnClassObject`).
//! Cold: `after_clause` calls in only for a class that inherits `Value`.
//! Every text lives here, so both lowerings raise the same errors.

use crate::mixin::MixinApplication;
use crate::types::*;
use crate::Symbol;

use super::{Mutator, Signal};

#[cold]
#[inline(never)]
pub(crate) fn check_value_class<'gc>(
    mutator: &Mutator<'gc>,
    name: Symbol,
    application: &MixinApplication,
    enclosing: ObjectReference<'gc>,
    superclass: ClassReference<'gc>,
) -> Result<(), Signal<'gc>> {
    let _ = (mutator, superclass);
    if let Some(declaring) = application.identity_method {
        return Err(Signal::Error(NewspeakError {
            selector: name,
            receiver: Value::Object(enclosing),
            message: format!(
                "cannot create {name}: a value class may not define ==, and {declaring} defines it"
            ),
        }));
    }
    Ok(())
}
