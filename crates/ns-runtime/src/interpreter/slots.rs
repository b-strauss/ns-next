//! Slot accessors: `newspeak-spec.pdf` §6.3.2. A slot's getter and a mutable
//! slot's setter are methods of its mixin -- "implicitly defined by the slot
//! declaration", with "the same" accessibility as the slot -- so every send
//! kind finds them by its ordinary lookup (`dispatch::walk`), and a
//! subclass overrides them like any method. Neither runs code: they are
//! answered here, without an activation, the interpreter's counterpart of
//! Cog's quick methods and SOMns's `CachedSlotRead`
//! (`docs/design/dispatch.md`, "Slot accessors").

use super::Arguments;
use crate::heap::ApplicationIndex;
use crate::mixin::{AccessorKind, DispatchId};
use crate::types::*;
use crate::Symbol;

use super::resolve::{Chain, Resolved};
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
    ///
    /// `#[inline(always)]`: with the guard check `act` makes ahead of it
    /// for a private or super accessor, LLVM called it out of line from
    /// every sender, public and protected accessors included.
    #[inline(always)]
    pub(crate) fn decide_slot_accessor(
        &mut self,
        receiver: Value<'gc>,
        kind: AccessorKind,
        selector: Symbol,
        field: usize,
        arguments: Arguments<'_, 'gc>,
    ) -> Result<Step<'gc>, Signal<'gc>> {
        if let Some(object) = receiver.as_object() {
            match kind {
                AccessorKind::Getter => {
                    if let Some(value) = self.heap.instance_field(object, field) {
                        return Ok(Step::Value(value));
                    }
                }
                AccessorKind::Setter => {
                    if let Some(value) = arguments.first(&self.heap) {
                        if self.heap.instance_set_field(object, field, value) {
                            return Ok(Step::Value(receiver));
                        }
                    }
                }
            }
        }
        Err(self.no_field_for_slot_accessor(receiver, selector, field))
    }

    /// Whether `receiver` still has the slots of `level`, the application
    /// whose own slot a private or super accessor addresses -- by a field
    /// the lookup decided from N's or `current_class`'s chain, never from
    /// the receiver's -- so that a receiver the bulk forward replaced by an
    /// object of another shape is never read or written in `level`'s
    /// layout (`docs/design/object-model.md`, "what a slot access checks").
    /// `act` asks it for `Chain::Private` and `Chain::Super` only, and
    /// raises `no_longer_has_the_slots` when it answers `false`; §5.10's
    /// slot handler is checked before `act` (`check_super_handler_slot`).
    ///
    /// A resolution the lookup makes carries `DispatchId::VACANT`, which no
    /// header holds -- the uncached path, the reference lowering's oracle, and
    /// a cached site's miss alike -- so it always walks and records nothing.
    /// Otherwise the hot path is one compare of the receiver's guard with the
    /// one the cache entry recorded;
    /// `DispatchId::NONE`, which an entry records for a receiver without
    /// the level, never matches. Next, the caches' one verified pair
    /// (`InlineCaches::slot_level_verified`), and only then
    /// `Heap::receiver_has_level`'s walk of the receiver's chain. What
    /// passes the walk is a receiver whose chain contains `level` under
    /// another guard -- an instance of another subclass at a site that
    /// served one before, or a receiver forwarded to an instance of a
    /// subclass -- and its guard and `level` are then recorded as the
    /// verified pair: a verification the walk made, not a decision. Without
    /// it a private slot of a class whose subclass's instances share its
    /// sites walked on every access -- DeltaBlue's `Plan`, a `List`, about
    /// ten million times a run, 3.9% of it. Rewriting the guard of the
    /// entry that hit instead carried the entry's location through every
    /// cached send, which slowed public and protected sends too, by
    /// 2.5-7.4%. A walk that fails records nothing. An immediate receiver
    /// has no slots to protect and passes, as an own-slot instruction's
    /// does; `decide_slot_accessor` answers its error.
    ///
    /// Out of line, answering a `bool` in a register, the compare included:
    /// with the compare inlined into `act` -- whose slot arm every sender
    /// inlines -- public and protected accessors slowed by 7-10%
    /// (`bench_loop.ns`, `bench_send.ns`) and private ones more than the
    /// call costs; a check answering a `Result` cost `DeltaBlue` 4%.
    #[inline(never)]
    pub(crate) fn slot_level_holds(
        &mut self,
        receiver: Value<'gc>,
        level: ApplicationIndex,
        recorded_guard: DispatchId,
    ) -> bool {
        let Some(object) = receiver.as_object() else {
            return true;
        };
        if recorded_guard == DispatchId::VACANT {
            return self.heap.receiver_has_level(receiver, level);
        }
        let guard = self.heap.object_dispatch_id(object);
        if recorded_guard != DispatchId::NONE && guard == recorded_guard {
            return true;
        }
        if self.caches.slot_level_verified(guard, level) {
            return true;
        }
        if !self.heap.receiver_has_level(receiver, level) {
            return false;
        }
        self.caches.record_slot_level(guard, level);
        true
    }

    /// §5.10's failure when S's `doesNotUnderstand:` handler is a slot's
    /// setter (§6.3.2, a mutable slot named `doesNotUnderstand`): its field
    /// is decided from S's chain without looking at the receiver, as a
    /// `super` accessor's is, so the receiver must still have the slots of
    /// the level it was found at, or the send raises the accessor's error.
    /// Such a handler is never cached (`inline_cache::record`), so this is
    /// the uncached check, walking every time; the uncached `super` send
    /// and a cached site's miss call it before `act`, which does not check
    /// this chain. Anything else passes.
    #[inline(always)]
    pub(crate) fn check_super_handler_slot(
        &self,
        resolved: &Resolved<'gc>,
        receiver: Value<'gc>,
    ) -> Result<(), Signal<'gc>> {
        match resolved {
            Resolved::Slot {
                chain: Chain::SuperDoesNotUnderstand,
                level,
                ..
            } => self.check_super_handler_level(receiver, *level),
            _ => Ok(()),
        }
    }

    /// `check_super_handler_slot`'s walk and error, out of line.
    #[cold]
    #[inline(never)]
    fn check_super_handler_level(
        &self,
        receiver: Value<'gc>,
        level: ApplicationIndex,
    ) -> Result<(), Signal<'gc>> {
        if receiver.as_object().is_none() || self.heap.receiver_has_level(receiver, level) {
            return Ok(());
        }
        Err(self.no_longer_has_the_slots(receiver, level, Symbol::DOES_NOT_UNDERSTAND))
    }

    /// The error of a private or super accessor whose receiver no longer
    /// has the slots of the level the accessor addresses.
    #[cold]
    #[inline(never)]
    pub(crate) fn no_longer_has_the_slots(
        &self,
        receiver: Value<'gc>,
        level: ApplicationIndex,
        selector: Symbol,
    ) -> Signal<'gc> {
        let class = self.heap.application(level).map_or_else(
            || "its class".to_string(),
            |level| level.mixin.reflected_name(),
        );
        Signal::Error(NewspeakError::error(
            selector,
            receiver,
            format!(
                "{} no longer has the slots of {class}, which the slot accessor {selector} addresses",
                self.heap.describe(receiver)
            ),
        ))
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
        Signal::Error(NewspeakError::error(
            selector,
            receiver,
            format!(
                "{} has no field {field} for the slot accessor {}",
                self.heap.describe(receiver),
                selector
            ),
        ))
    }
}
