//! What a heap object names by a `SmallInteger` because it cannot live in
//! the words: a mixin, a mixin application, a compiled body
//! (`docs/design/object-model.md`, "The object layout"). Per heap, because
//! the objects naming them are, and append-only: road step 3a mints these
//! ids and reclaims none, since nothing drops a mixin, a superseded version
//! included, before road step 5b2c's reclamation (`docs/OPEN-WORK.md`,
//! "Next").
//!
//! A mixin's id is its identity: every version of it answers the id it was
//! first registered under, and the row holds the current version and the
//! id of the mixin it is declared in, so that an edit
//! installs a fresh mixin under the old id and everything that names the
//! mixin by id sees the edited one
//! (`docs/design/liveness-and-reflection.md`, "An edit replaces the mixin
//! under its id").

use std::rc::Rc;

use crate::bytecode::CompiledCode;
use crate::hash::IdentityMap;
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

/// The three tables. The address maps are keyed by `Rc::as_ptr`, which is
/// sound only because the table holds the strong `Rc` that keeps each
/// address valid and unique for as long as the entry lives -- forever, in
/// 3a. `mixin_ids` names every version of a mixin, so every version is
/// held: the current one in `mixins`, each earlier one in `superseded`,
/// until road step 5b2c's weak application cache decides which of them may
/// be dropped.
#[derive(Default)]
pub(crate) struct Tables {
    /// Per id, the mixin's current version.
    pub(crate) mixins: Vec<Rc<Mixin>>,
    /// The address of every version of every mixin to its id.
    pub(crate) mixin_ids: IdentityMap<usize, MixinId>,
    /// Every version a replacement superseded, held so that its address
    /// stays unique while `mixin_ids` names it.
    pub(crate) superseded: Vec<Rc<Mixin>>,
    /// Per id, the id of the mixin it is declared in: a nested
    /// declaration's enclosing declaration, a class side's instance side
    /// (`Mixin::enclosing`), read once, when the mixin is registered, and
    /// kept by a replacement.
    pub(crate) enclosing: Vec<Option<MixinId>>,
    pub(crate) applications: Vec<Rc<MixinApplication>>,
    /// Per application an install replaced, the application of the edited
    /// mixin that took its place, always the newest of its chain
    /// (`Heap::record_successor` repoints the earlier entries): an
    /// activation entered at the old level passes the slot check's cold
    /// path against the new one, because an edit that reaches the install
    /// keeps the level's slots. No entry is dropped; the rule for reusing
    /// an application's index is road step 5b2c's.
    pub(crate) successors: IdentityMap<ApplicationIndex, ApplicationIndex>,
    pub(crate) codes: Vec<CodeEntry>,
    pub(crate) code_ids: IdentityMap<usize, CodeId>,
}
