//! The leaves through which shipped Newspeak reaches another shipped module
//! (`docs/design/core-library.md`, "The module table and the boot"):
//! `PlatformFactory.ns` reaches the other modules with them.
//!
//! `moduleInstanceNamed:` answers the canonical instance recorded in the
//! shipped world (`interpreter::shipped_world`), the same object on every
//! request, so every platform shares one collections module. The boot
//! records only the kernel and `PlatformFactory`; every other module is
//! built on its first demand, in a lazy slot of `PlatformFactory.ns`, with
//! ordinary sends between the other two: `shippedModuleClassNamed:`
//! answers its root class, the module's factory is sent to it, and
//! `recordShippedModule:` records what the factory answered as the
//! module's one instance. None of the three runs Newspeak, and nothing is
//! recorded before the factory answered, so a demand an unwind interrupts
//! leaves the next one to build the module from the same class.

use crate::builtins::primitive::ModulePrimitive;
use crate::interpreter::core_library::{record_pending_module, shipped_module_class};
use crate::types::*;
use crate::Mutator;

/// Declines -- and the forwarder's failure block runs -- when the subject
/// is no symbol or names no shipped module (`InstanceNamed`), names no
/// lazy shipped module not yet recorded (`ClassNamed`), or is no instance
/// of a pending root class (`Record`).
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
        ModulePrimitive::ClassNamed => {
            let name = receiver.as_object()?;
            let name = mutator.heap().symbol_value(name)?;
            let class = shipped_module_class(mutator, name)?;
            Some(Ok(Value::object(class)))
        }
        ModulePrimitive::Record => {
            let instance = record_pending_module(mutator, receiver)?;
            Some(Ok(Value::object(instance)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Symbol;

    fn class_named<'gc>(
        mutator: &mut Mutator<'gc>,
        subject: Value<'gc>,
    ) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
        execute(mutator, ModulePrimitive::ClassNamed, subject)
    }

    fn record<'gc>(
        mutator: &mut Mutator<'gc>,
        subject: Value<'gc>,
    ) -> Option<Result<Value<'gc>, NewspeakError<'gc>>> {
        execute(mutator, ModulePrimitive::Record, subject)
    }

    /// The kernel and the platform are built at boot, and a name that is
    /// no shipped module's, or no symbol, names nothing to build.
    #[test]
    fn the_class_leaf_declines_a_name_that_is_no_lazy_module() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.enter(|mutator| {
            for name in ["Kernel", "PlatformFactory", "Nothing"] {
                let symbol = mutator.new_symbol(Symbol::intern(name));
                assert!(
                    class_named(mutator, symbol).is_none(),
                    "shippedModuleClassNamed: #{name} declines"
                );
            }
            let string = Value::object(mutator.new_string("CollectionsFactory".to_string()));
            assert!(class_named(mutator, string).is_none());
            assert!(class_named(mutator, Value::nil()).is_none());
        });
    }

    /// A module already recorded has its one instance, so there is no
    /// class left to build. The boot builds no lazy module, so the host
    /// builds one first.
    #[test]
    fn the_class_leaf_declines_a_recorded_module() {
        let mut interpreter = crate::Interpreter::bootstrap();
        interpreter.build_shipped_module("CollectionsFactory", None);
        interpreter.enter(|mutator| {
            let name = mutator.new_symbol(Symbol::intern("CollectionsFactory"));
            assert!(mutator
                .shipped_world
                .module_instance(Symbol::intern("CollectionsFactory"))
                .is_some());
            assert!(class_named(mutator, name).is_none());
        });
    }

    /// Only an instance of a pending root class is recorded: neither `nil`,
    /// nor a fresh `Object`, nor a module instance already recorded adds a
    /// name to the shipped world.
    #[test]
    fn the_record_leaf_declines_what_is_no_instance_of_a_pending_class() {
        let mut interpreter = crate::Interpreter::bootstrap();
        let object_class = interpreter.enter(|mutator| {
            let class = mutator.builtin_class(BuiltinClass::Object);
            mutator.stash(Value::object(class))
        });
        let object = match interpreter.send(&object_class, crate::Symbol::NEW, &[]) {
            Ok(object) => object,
            Err(crate::HostSignal::Return) => panic!("Object new: a non-local return escaped"),
            Err(crate::HostSignal::Error(error)) => {
                panic!("Object new: unexpected Newspeak error: {}", error.message)
            }
        };
        interpreter.enter(|mutator| {
            let before = mutator.shipped_world.entries().len();
            let object = mutator.fetch(&object);
            let kernel = mutator
                .shipped_world
                .module_instance(Symbol::intern("Kernel"))
                .expect("the kernel is recorded at boot");
            for subject in [Value::nil(), object, Value::object(kernel)] {
                assert!(record(mutator, subject).is_none());
            }
            assert_eq!(mutator.shipped_world.entries().len(), before);
        });
    }
}
