//! `moduleInstanceNamed:`, the leaf through which shipped Newspeak reaches
//! another shipped module (`docs/design/core-library.md`, "The module table
//! and the boot"): `PlatformFactory.ns` reaches the collections module with
//! it.
//!
//! It answers the canonical instance the boot built and recorded in the
//! shipped world (`interpreter::shipped_world`), the same object on every
//! request, so every platform shares one collections module. It builds
//! nothing and runs no Newspeak.

use crate::builtins::primitive::ModulePrimitive;
use crate::types::*;
use crate::Mutator;

/// Declines -- and the forwarder's failure block runs -- when the subject
/// is no symbol or names no shipped module.
pub(crate) fn execute<'gc>(
    mutator: &mut Mutator<'gc>,
    primitive: ModulePrimitive,
    receiver: Value<'gc>,
) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
    match primitive {
        ModulePrimitive::InstanceNamed => {
            let name = receiver.as_object()?;
            let name = mutator.heap().symbol_value(name)?;
            let instance = mutator.shipped_world.module_instance(name)?;
            Some(Ok(Value::object(instance)))
        }
    }
}
