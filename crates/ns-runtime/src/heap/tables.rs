//! What a heap object names by a `SmallInteger` because it cannot live in
//! the words: a mixin, a mixin application, a compiled body
//! (`docs/design/object-model.md`, "Planned: the object layout"). Per heap,
//! because the objects naming them are, and append-only: road step 3a mints
//! these ids and reclaims none, since nothing drops a mixin before live
//! editing (`docs/OPEN-WORK.md`, "Live editing of mixins").
//!
//! A mixin's id is its identity: every version of it answers the id it was
//! first registered under, and the row holds the current version, the id of
//! the mixin it is declared in and its module's text, so that an edit
//! installs a fresh mixin under the old id and everything that names the
//! mixin by id sees the edited one
//! (`docs/design/liveness-and-reflection.md`, "Planned with road step 5: an
//! edit replaces the mixin under its id").

use std::rc::Rc;

use crate::bytecode::CompiledCode;
use crate::hash::IdentityMap;
use crate::mixin::{Mixin, MixinApplication, MixinMethod, ModuleText};

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
/// until road step 5b2's weak application cache decides which of them may
/// be dropped.
#[derive(Default)]
pub(crate) struct Tables {
    /// Per id, the mixin's current version.
    pub(crate) mixins: Vec<Rc<Mixin>>,
    /// The address of every version of every mixin to its id.
    pub(crate) mixin_ids: IdentityMap<usize, MixinId>,
    /// Every version a replacement superseded, held so that its address
    /// stays unique while `mixin_ids` names it.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "road step 5b2's install replaces a mixin; until then only the tests do"
        )
    )]
    pub(crate) superseded: Vec<Rc<Mixin>>,
    /// Per id, the id of the mixin it is declared in: a nested
    /// declaration's enclosing declaration, a class side's instance side
    /// (`Mixin::enclosing`), read once, when the mixin is registered, and
    /// kept by a replacement.
    pub(crate) enclosing: Vec<Option<MixinId>>,
    /// Per id, the text of the module the current version is read in
    /// (`Mixin::module_text`), read when the mixin is registered and given
    /// anew by a replacement.
    pub(crate) module_texts: Vec<Option<ModuleText>>,
    pub(crate) applications: Vec<Rc<MixinApplication>>,
    pub(crate) codes: Vec<CodeEntry>,
    pub(crate) code_ids: IdentityMap<usize, CodeId>,
}
