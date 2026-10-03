//! What a heap object names by a `SmallInteger` because it cannot live in
//! the words: a mixin, a mixin application, a compiled body
//! (`docs/design/object-model.md`, "Planned: the object layout"). Per heap,
//! because the objects naming them are, and append-only: road step 3a mints
//! these ids and reclaims none, since nothing drops a mixin before live
//! editing (`docs/OPEN-WORK.md`, "Live editing of mixins").

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
/// 3a.
#[derive(Default)]
pub(crate) struct Tables {
    pub(crate) mixins: Vec<Rc<Mixin>>,
    pub(crate) mixin_ids: IdentityMap<usize, MixinId>,
    pub(crate) applications: Vec<Rc<MixinApplication>>,
    pub(crate) codes: Vec<CodeEntry>,
    pub(crate) code_ids: IdentityMap<usize, CodeId>,
}
