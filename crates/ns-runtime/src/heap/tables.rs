//! What a heap object names by a `SmallInteger` because it cannot live in
//! the words: a mixin, a mixin application, a compiled body
//! (`docs/design/object-model.md`, "The object layout"). Per heap, because
//! the objects naming them are. Every table gives its rows back: a full
//! collection frees the row of every application, every module's tree of
//! mixins and every compiled body nothing names any more
//! (`interpreter::reclaim`), and the next registration takes the smallest
//! free row, so that a table stays dense and its trailing free rows are
//! given back.
//!
//! A mixin's id is its identity: every version of it answers the id it was
//! first registered under, and the row holds the current version, the id
//! of the mixin it is declared in and the ids of those declared in it, so
//! that an edit
//! installs a fresh mixin under the old id and everything that names the
//! mixin by id sees the edited one
//! (`docs/design/liveness-and-reflection.md`, "An edit replaces the mixin
//! under its id"). A module's mixins live and die together, as one tree:
//! a live mixin keeps the ids of those it is declared in and of those
//! declared in it.

use std::rc::Rc;

use crate::bytecode::CompiledCode;
use crate::hash::IdentityMap;
use crate::heap::FreeIndices;
use crate::mixin::{Mixin, MixinApplication, MixinMethod};

macro_rules! table_index {
    ($name:ident, $what:literal) => {
        #[doc = concat!("The index of ", $what, " in its heap's table.")]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub struct $name(u32);

        impl $name {
            pub fn index(self) -> usize {
                self.0 as usize
            }

            /// `None` past `u32::MAX`, which a table cannot reach before
            /// memory runs out.
            pub(crate) fn from_index(index: usize) -> Option<$name> {
                u32::try_from(index).ok().map($name)
            }
        }
    };
}

table_index!(MixinId, "a mixin");
table_index!(ApplicationIndex, "a mixin application");
table_index!(CodeId, "a compiled body");

/// A body's code id in the last heap that asked for it, with that heap's
/// code-table epoch (`Heap::code_table_epoch`): a one-entry cache in front of
/// `Heap::register_code`'s map, read by every `MakeClosure` and every
/// frame reified. The epoch is unique per heap of the process and renewed
/// whenever a full collection frees code rows, so an id from another heap,
/// or one since freed, is never answered. A copy of a body is another body
/// to the map, keyed by its address, so a copy starts empty.
#[derive(Default)]
pub(crate) struct CodeIdCache(std::cell::Cell<Option<(u64, CodeId)>>);

impl CodeIdCache {
    pub(crate) fn get(&self) -> Option<(u64, CodeId)> {
        self.0.get()
    }

    pub(crate) fn set(&self, epoch: u64, id: CodeId) {
        self.0.set(Some((epoch, id)));
    }
}

impl Clone for CodeIdCache {
    fn clone(&self) -> CodeIdCache {
        CodeIdCache::default()
    }
}

impl std::fmt::Debug for CodeIdCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CodeIdCache")
    }
}

impl ApplicationIndex {
    /// What an application carries until `Heap::register_application`
    /// stamps it; one built outside `Mutator::apply_mixin` (a test helper)
    /// keeps it.
    pub(crate) const UNREGISTERED: ApplicationIndex = ApplicationIndex(u32::MAX);
}

/// A compiled body and, for a method's, the method -- so that a method
/// frame's code id names its method too, which the encoder writes and
/// `image::decode::frame_body` reads.
pub(crate) struct CodeEntry {
    pub(crate) code: Rc<CompiledCode>,
    pub(crate) method: Option<Rc<MixinMethod>>,
}

/// The current version of a registered mixin and where it stands in its
/// module's tree.
pub(crate) struct MixinRow {
    /// The mixin's current version.
    pub(crate) current: Rc<Mixin>,
    /// The id of the mixin it is declared in: a nested declaration's
    /// enclosing declaration, a class side's instance side
    /// (`Mixin::enclosing`), read once, when the mixin is registered, and
    /// kept by a replacement.
    pub(crate) enclosing: Option<MixinId>,
    /// The ids of the declarations nested directly in the mixin, in
    /// declaration order, the same for every version of it
    /// (`Heap::nested_of`): registration takes them out of the declared
    /// tree (`Mixin::take_declared_nested`), so a mixin holds no other
    /// mixin's `Rc` once a heap names it.
    pub(crate) nested: Vec<MixinId>,
}

/// The tables. The address maps are keyed by `Rc::as_ptr`, which is sound
/// only because the table holds the strong `Rc` that keeps each address
/// valid and unique for as long as the entry lives: a version or a body
/// leaves its map when the table lets go of it, so an address the
/// allocator hands out again is never found under a key that named its
/// earlier occupant. `mixin_ids` names every version of a mixin the tables
/// hold: the current one in its row, each earlier one in `superseded`
/// until nothing but the table holds it and nothing runs code of it.
#[derive(Default)]
pub(crate) struct Tables {
    /// Per id, the mixin's row, or `None` for an id a collection freed
    /// with its module's tree.
    pub(crate) mixins: Vec<Option<MixinRow>>,
    /// The free rows of `mixins`, minted smallest first.
    pub(crate) free_mixins: FreeIndices,
    /// The address of every version the tables hold to its id.
    pub(crate) mixin_ids: IdentityMap<usize, MixinId>,
    /// Every version a replacement superseded that may still be held -- by
    /// an application built from it, a host, a nested class's creation
    /// underway -- kept so that its address stays unique while `mixin_ids`
    /// names it. A collection drops a version once only this table holds it
    /// and no running body -- of a closure, a frame or an activation -- was
    /// compiled in it (`Heap::drop_superseded`).
    pub(crate) superseded: Vec<Rc<Mixin>>,
    /// Per index, the application, or `None` for a row a collection freed
    /// (`Heap::free_application`).
    pub(crate) applications: Vec<Option<Rc<MixinApplication>>>,
    /// The free rows of `applications`, minted smallest first.
    pub(crate) free_applications: FreeIndices,
    /// Per application an install replaced, the application of the edited
    /// mixin that took its place, always the newest of its chain
    /// (`Heap::record_successor` repoints the earlier entries): an
    /// activation entered at the old level passes the slot check's cold
    /// path against the new one, because an edit that reaches the install
    /// keeps the level's slots. An entry lives while its old index does,
    /// and keeps its new one alive; a collection drops it with the old
    /// index (`Heap::retain_successors`), so an index minted again never
    /// inherits a successor.
    pub(crate) successors: IdentityMap<ApplicationIndex, ApplicationIndex>,
    /// Per id, the body, or `None` for an id a collection freed because
    /// no closure or frame named it any more.
    pub(crate) codes: Vec<Option<CodeEntry>>,
    /// The free rows of `codes`, minted smallest first.
    pub(crate) free_codes: FreeIndices,
    /// The address of every body the table holds to its id.
    pub(crate) code_ids: IdentityMap<usize, CodeId>,
}
