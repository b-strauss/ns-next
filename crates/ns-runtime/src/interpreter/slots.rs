//! Slot accessors: `newspeak-spec.pdf` §6.3.2. A slot's getter and a mutable
//! slot's setter are methods of its mixin -- "implicitly defined by the slot
//! declaration", with "the same" accessibility as the slot -- so every send
//! kind finds them by its ordinary lookup (`dispatch::walk`), and a
//! subclass overrides them like any method. Neither runs code: they are
//! answered here, without an activation, the interpreter's counterpart of
//! Cog's quick methods and SOMns's `CachedSlotRead`
//! (`docs/design/dispatch.md`, "Slot accessors").

use crate::mixin::AccessorKind;
use crate::types::*;
use crate::Symbol;

use super::{Mutator, Signal, Step};

impl<'gc> Mutator<'gc> {
    /// Reads or writes `field` of `receiver`, the absolute field the lookup
    /// resolved the accessor's mixin-relative slot to; `selector` is the
    /// accessor's own, which the lookup matched it by. The getter "returns
    /// the value of the slot"; the setter "sets the value of the slot to be
    /// the incoming argument. It returns the receiver of the message"
    /// (§6.3.2).
    ///
    /// A receiver without that field is a Newspeak error, never a panic: an
    /// accessor is found only on its own class's instances, so only a decoded
    /// image whose layout disagrees with its mixins -- untrusted input -- can
    /// get here without one.
    pub(crate) fn decide_slot_accessor(
        &mut self,
        receiver: Value<'gc>,
        kind: AccessorKind,
        selector: Symbol,
        field: usize,
        arguments: &[Value<'gc>],
    ) -> Result<Step<'gc>, Signal<'gc>> {
        if let Some(object) = receiver.as_object() {
            match kind {
                AccessorKind::Getter => {
                    if let Some(value) = self.heap.instance_field(object, field) {
                        return Ok(Step::Value(value));
                    }
                }
                AccessorKind::Setter => {
                    if let Some(value) = arguments.first() {
                        if self.heap.set_instance_field(object, field, *value) {
                            return Ok(Step::Value(receiver));
                        }
                    }
                }
            }
        }
        Err(self.no_field_for_slot_accessor(receiver, selector, field))
    }

    /// `decide_slot_accessor`'s error, out of line: formatted in place, it
    /// gave the accessor -- every slot read and write of a program -- a
    /// stack frame through which the field it read was spilled and reloaded
    /// before it reached the step, and that cost `bench_send` 18% and
    /// `bench_loop` 25% (`docs/design/execution.md`, "What exceptions
    /// cost").
    #[cold]
    #[inline(never)]
    fn no_field_for_slot_accessor(
        &self,
        receiver: Value<'gc>,
        selector: Symbol,
        field: usize,
    ) -> Signal<'gc> {
        Signal::Error(NewspeakError {
            selector,
            receiver,
            message: format!(
                "{} has no field {field} for the slot accessor {}",
                self.heap.describe(receiver),
                selector
            ),
        })
    }
}
