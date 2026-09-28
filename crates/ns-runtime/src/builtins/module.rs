//! `moduleNamed:`, the leaf through which shipped Newspeak reaches another
//! shipped module (`docs/design/core-library.md`, "The module table and the
//! boot"): `PlatformFactory.ns` builds the collections module with it.
//!
//! It answers the module's root class, built from the table's resolved
//! source by `Mutator::load_root_class`, which is pure structure and runs no
//! Newspeak. It keeps nothing: every request builds a new class object, and
//! the caller keeps the one it wants -- `PlatformFactory`'s root in a lazy
//! slot, which runs once -- so the runtime holds no module root of its own
//! and traces nothing more.

use crate::builtins::primitive::ModulePrimitive;
use crate::interpreter::core_library;
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
        ModulePrimitive::Named => {
            let Value::Object(name) = receiver else {
                return None;
            };
            let name = mutator.heap().symbol_value(name)?;
            let resolved = core_library::shipped_module_named(name.as_str())?;
            let class = mutator.load_root_class(&resolved);
            Some(Ok(Value::Object(class)))
        }
    }
}
