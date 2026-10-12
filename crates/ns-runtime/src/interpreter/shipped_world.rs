//! The shipped world: every object the binary owns and an image may name
//! (`docs/design/image-format.md`, "Planned: shipped names in the image").
//! Filled once, at the end of the boot, and never changed afterwards.

use crate::trace::{Trace, Tracer};
use crate::types::ObjectReference;
use crate::Symbol;

/// The shipped modules' instances, in the order the boot built them: the
/// kernel first.
#[derive(Default)]
pub(crate) struct ShippedWorld<'gc> {
    module_instances: Vec<(Symbol, ObjectReference<'gc>)>,
}

impl<'gc> ShippedWorld<'gc> {
    /// Records `instance` as the one instance of the shipped module `name`.
    /// The boot names each module once; a second instance is a defect in
    /// the binary.
    pub(crate) fn add_module_instance(&mut self, name: Symbol, instance: ObjectReference<'gc>) {
        assert!(
            self.module_instance(name).is_none(),
            "internal: the shipped module {} is instantiated twice",
            name.as_str()
        );
        self.module_instances.push((name, instance));
    }

    /// The one instance of the shipped module `name`, or `None` when no
    /// shipped module has that name.
    pub(crate) fn module_instance(&self, name: Symbol) -> Option<ObjectReference<'gc>> {
        self.module_instances()
            .iter()
            .find(|(module, _)| *module == name)
            .map(|(_, instance)| *instance)
    }

    /// Every shipped module's name and instance, in the order the boot
    /// built them.
    pub(crate) fn module_instances(&self) -> &[(Symbol, ObjectReference<'gc>)] {
        &self.module_instances
    }
}

impl Trace for ShippedWorld<'_> {
    fn trace<T: Tracer>(&mut self, tracer: &mut T) {
        for (_, instance) in &mut self.module_instances {
            tracer.object(instance);
        }
    }
}
