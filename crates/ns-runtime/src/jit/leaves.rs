//! The Rust functions machine code calls through the leaf shim, each an
//! instruction's own interpreter step that runs no Newspeak code, cannot
//! collect and reads no other activation (`docs/design/execution.md`,
//! "The foundation and the one extension point", the leaf call). A
//! template names one by its index in `table` (`LeafIndex`).

use ns_memory::Leaf;

use crate::interpreter::Mutator;

/// The leaves, in `LeafIndex` order, each handed the mutator the code was
/// entered with. None yet: every instruction lowers to an exit.
pub(crate) fn table<'gc>() -> [Leaf<Mutator<'gc>>; 0] {
    []
}
