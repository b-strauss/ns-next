//! What a full collection reads and frees to give the per-heap ids back
//! (`docs/design/object-model.md`, "An application has two ids"): the ids
//! marking finds named, the roots the activation stack holds outside the
//! heap, and the table operations `Mutator::reclaim_ids` decides with.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use super::value::{Value, Word};
use super::{ActivationReference, ApplicationIndex, CodeId, Heap, MixinId, Tables};
use crate::hash::IdentityBuildHasher;
use crate::interpreter::FeedbackBase;
use crate::mixin::{mixin_key, AppliedClause, DispatchId, Mixin, MixinApplication};

/// What a full marking found named: one bit per row of each table.
pub(crate) struct GatheredIds {
    /// The length of the application, mixin and code tables when marking
    /// began. A word naming a row at or past it names nothing, since the
    /// gathered rows are only compared with table rows; dropping it keeps a
    /// crafted image's `u32::MAX` row from sizing a bit set at 512 MiB.
    application_rows: usize,
    mixin_rows: usize,
    code_rows: usize,
    /// `class_field::APPLICATION` of every marked class object.
    pub(crate) applications: BitSet,
    /// The mixin id `class_field::APPLIED_CLAUSE` packs, of every marked
    /// class object that has one.
    pub(crate) applied_clauses: BitSet,
    /// `CODE_ID` of every marked closure and frame.
    pub(crate) codes: BitSet,
    /// `FEEDBACK_BASE` of every marked closure, raw, but for
    /// `FeedbackBase::NOT_ASSIGNED`.
    pub(crate) feedback_bases: BTreeSet<u32>,
}

impl GatheredIds {
    /// An empty gathering for tables of the given lengths.
    pub(super) fn new(application_rows: usize, mixin_rows: usize, code_rows: usize) -> GatheredIds {
        GatheredIds {
            application_rows,
            mixin_rows,
            code_rows,
            applications: BitSet::default(),
            applied_clauses: BitSet::default(),
            codes: BitSet::default(),
            feedback_bases: BTreeSet::new(),
        }
    }

    /// A marked class object's application and applied-clause words, as
    /// stored. A word that is no `SmallInteger` names nothing: a crafted
    /// image's, which the readers of the field refuse the same way.
    pub(super) fn class_words(&mut self, application: u64, applied_clause: u64) {
        if let Some(row) = small_integer(application)
            .and_then(row_of)
            .filter(|row| (*row as usize) < self.application_rows)
        {
            self.applications.insert(row);
        }
        if let Some((mixin, _, _)) = small_integer(applied_clause).and_then(AppliedClause::unpack) {
            if let Ok(row) = u32::try_from(mixin.index()) {
                if (row as usize) < self.mixin_rows {
                    self.applied_clauses.insert(row);
                }
            }
        }
    }

    /// A marked closure's code-id and feedback-base words, as stored.
    pub(super) fn closure_words(&mut self, code: u64, feedback_base: u64) {
        self.frame_word(code);
        if let Some(raw) = small_integer(feedback_base).and_then(row_of) {
            if raw != FeedbackBase::NOT_ASSIGNED.raw() {
                self.feedback_bases.insert(raw);
            }
        }
    }

    /// The code ids the running activations name (`StackRoots::codes`),
    /// which no marking reaches: every one names a live row, since a
    /// running activation's row is never freed.
    pub(crate) fn running_codes(&mut self, codes: &[CodeId]) {
        for code in codes {
            self.codes
                .insert(u32::try_from(code.index()).expect("a code row fits a u32"));
        }
    }

    /// A marked frame's code-id word, as stored: nil for a top-level
    /// frame's, which names no body.
    pub(super) fn frame_word(&mut self, code: u64) {
        if let Some(row) = small_integer(code)
            .and_then(row_of)
            .filter(|row| (*row as usize) < self.code_rows)
        {
            self.codes.insert(row);
        }
    }
}

/// The `SmallInteger` a stored word holds, if it holds one: the decoding
/// `Heap::load` makes, without the heap, which marking holds borrowed.
fn small_integer(bits: u64) -> Option<i64> {
    Value::from_word(
        Word::from_bits(bits),
        #[cfg(debug_assertions)]
        0,
        #[cfg(debug_assertions)]
        0,
    )
    .as_small_integer()
}

/// A table row a stored number names, if it can be one.
fn row_of(number: i64) -> Option<u32> {
    u32::try_from(number).ok()
}

/// A growable set of `u32` rows, a word per 64.
#[derive(Default)]
pub(crate) struct BitSet {
    words: Vec<u64>,
}

impl BitSet {
    pub(crate) fn insert(&mut self, index: u32) {
        let word = (index / 64) as usize;
        if self.words.len() <= word {
            self.words.resize(word + 1, 0);
        }
        self.words[word] |= 1 << (index % 64);
    }

    pub(crate) fn contains(&self, index: u32) -> bool {
        self.words
            .get((index / 64) as usize)
            .is_some_and(|word| word & (1 << (index % 64)) != 0)
    }
}

/// What a reclamation freed, for the caches' scrub.
#[derive(Default)]
pub(crate) struct FreedIds {
    pub(crate) guards: HashSet<DispatchId, IdentityBuildHasher>,
    pub(crate) applications: HashSet<ApplicationIndex, IdentityBuildHasher>,
}

impl FreedIds {
    pub(crate) fn is_empty(&self) -> bool {
        self.guards.is_empty() && self.applications.is_empty()
    }
}

/// What the live activations name outside the heap's objects: no
/// marking reaches these, since a stack frame holds them as numbers, and
/// a suspended activation's row of the side table as well.
pub(crate) struct StackRoots {
    /// Every activation's `entered_level`, a suspended one's included,
    /// `UNREGISTERED` included.
    pub(crate) entered_levels: Vec<ApplicationIndex>,
    /// Every activation's `recorded_guard`, `NONE` included.
    pub(crate) recorded_guards: Vec<DispatchId>,
    /// Every activation's feedback base, `NOT_ASSIGNED` included.
    pub(crate) feedback_bases: Vec<FeedbackBase>,
    /// Every running activation's code id. A suspended activation's is its
    /// frame's `CODE_ID`, which marking gathers: the side table of
    /// suspensions is a root, so the frame is always marked.
    pub(crate) codes: Vec<CodeId>,
}

impl Heap<'_> {
    /// Clears every reified mixin object the marking did not reach. The
    /// table is weak: while anything reaches the object it is marked and
    /// stays the one object of its mixin, so `==` stays the mixin's
    /// identity for every comparison a program can make; once nothing
    /// reaches it, nothing can compare it, and the next reflection makes a
    /// fresh one. Between marking and compaction, as
    /// `prune_canonical_symbols` is, for the same reason.
    pub(crate) fn prune_mixin_objects(&mut self) {
        for index in 0..self.mixin_objects.len() {
            if let Some(object) = self.mixin_objects[index] {
                if !self.header(object).marked() {
                    self.mixin_objects[index] = None;
                }
            }
        }
    }

    /// The ids every live activation names (`StackRoots`). A suspended
    /// one records no guard and keeps no feedback base: it resumes with
    /// none and with its base derived again.
    pub(crate) fn stack_roots(&self) -> StackRoots {
        let running: Vec<ActivationReference> = self
            .stack
            .offsets()
            .map(|offset| self.activation_handle(offset))
            .collect();
        let suspended = self.suspensions.rows.iter().flatten();
        StackRoots {
            entered_levels: running
                .iter()
                .map(|activation| self.activation_entered_level(*activation))
                .chain(suspended.map(|suspension| suspension.entered_level))
                .collect(),
            recorded_guards: running
                .iter()
                .map(|activation| self.activation_receiver_and_recorded_guard(*activation).1)
                .collect(),
            feedback_bases: running
                .iter()
                .map(|activation| self.activation_feedback_base(*activation))
                .collect(),
            codes: running
                .iter()
                .map(|activation| self.activation_code_id(*activation))
                .collect(),
        }
    }

    /// How many rows the application table has, free ones included.
    pub(crate) fn application_rows(&self) -> usize {
        self.tables.applications.len()
    }

    /// How many rows the mixin table has, free ones included.
    // Read by the tests that bound the tables' growth.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn mixin_rows(&self) -> usize {
        self.tables.mixins.len()
    }

    /// How many superseded versions the tables hold.
    // Read by the tests that bound the tables' growth.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn superseded_count(&self) -> usize {
        self.tables.superseded.len()
    }

    /// How many rows the code table has, free ones included.
    // Read by the tests that bound the tables' growth.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn code_rows(&self) -> usize {
        self.tables.codes.len()
    }

    /// How many replaced applications name a successor.
    #[cfg(test)]
    pub(crate) fn successor_count(&self) -> usize {
        self.tables.successors.len()
    }

    /// Takes the application out of row `index` and frees the index, which
    /// the next `register_application` may mint again. A compiler-invariant
    /// violation panics: freeing a row that holds nothing.
    pub(crate) fn free_application(&mut self, index: ApplicationIndex) -> Rc<MixinApplication> {
        let application = self
            .tables
            .applications
            .get_mut(index.index())
            .and_then(Option::take)
            .expect("internal: a freed application row holds an application");
        self.tables
            .free_applications
            .free(u32::try_from(index.index()).expect("an application index is 32 bits"));
        application
    }

    /// Drops every successor entry whose replaced application `live` calls
    /// dead: its index is about to be freed, and an entry left under it
    /// would hand the next application minted there a successor of its own.
    pub(crate) fn retain_successors(&mut self, live: impl Fn(ApplicationIndex) -> bool) {
        self.tables.successors.retain(|old, _| live(*old));
    }

    /// Gives back the application table's trailing free rows.
    pub(crate) fn truncate_application_rows(&mut self) {
        super::ids::truncate_trailing(
            &mut self.tables.applications,
            &mut self.tables.free_applications,
        );
    }

    /// The id of every live application's mixin, a superseded version's
    /// included, which answers its id as the current one does.
    pub(crate) fn applied_mixin_ids(&self) -> BitSet {
        let mut named = BitSet::default();
        for application in self.tables.applications.iter().flatten() {
            if let Some(id) = self.known_mixin_id(&application.mixin) {
                named.insert(mixin_row_number(id));
            }
        }
        named
    }

    /// The id of every mixin whose reified object survived the prune
    /// (`prune_mixin_objects`): a program can still compare it.
    pub(crate) fn reified_mixin_ids(&self) -> BitSet {
        let mut named = BitSet::default();
        for (index, object) in self.mixin_objects.iter().enumerate() {
            if object.is_some() {
                named.insert(u32::try_from(index).expect("a mixin row fits a u32"));
            }
        }
        named
    }

    /// Every registered module's tree of mixin ids, each its root first: a
    /// row belongs to the tree of the root its chain of enclosing mixins
    /// ends at, so a tree holds the root's nested declarations at every
    /// depth, the class sides registered under any of them, and a nested
    /// declaration an install removed, whose row still names the mixin it
    /// was declared in. Every row is in exactly one tree.
    pub(crate) fn module_trees(&self) -> Vec<Vec<MixinId>> {
        let rows = self.tables.mixins.len();
        // Per row, the index of its tree in `trees`; walked, not recursed,
        // because an image chooses how deep its declarations nest.
        let mut tree_of: Vec<Option<usize>> = vec![None; rows];
        let mut trees: Vec<Vec<MixinId>> = Vec::new();
        let mut path = Vec::new();
        for row in 0..rows {
            let id = mixin_id_of_row(row);
            if self.mixin_row(id).is_none() || tree_of[row].is_some() {
                continue;
            }
            let mut current = id;
            let tree = loop {
                if let Some(tree) = tree_of[current.index()] {
                    break tree;
                }
                path.push(current);
                match self.enclosing_mixin_id(current) {
                    Some(enclosing) if self.mixin_row(enclosing).is_some() => {
                        debug_assert!(path.len() <= rows, "internal: a cycle of enclosing mixins");
                        current = enclosing;
                    }
                    enclosing => {
                        debug_assert!(
                            enclosing.is_none(),
                            "internal: a mixin's enclosing mixin has a row"
                        );
                        trees.push(Vec::new());
                        break trees.len() - 1;
                    }
                }
            };
            for member in path.drain(..) {
                tree_of[member.index()] = Some(tree);
            }
        }
        for (row, tree) in tree_of.iter().enumerate() {
            if let Some(tree) = tree {
                trees[*tree].push(mixin_id_of_row(row));
            }
        }
        for tree in &mut trees {
            // The root first: the only row of its tree with no enclosing.
            if let Some(root) = tree
                .iter()
                .position(|id| self.enclosing_mixin_id(*id).is_none())
            {
                tree[..=root].rotate_right(1);
            }
        }
        trees
    }

    /// The ids among `trees`' rows of which some version, current or
    /// superseded, is held outside the tables: by a host, an activation's
    /// nested-class creation, a cache entry. Such a holder can present the
    /// version again, and must find it under the same id, so its tree
    /// lives.
    ///
    /// A version's own holders inside the tables are counted rather than
    /// listed: its row or its place in `superseded`, the `class_side` of
    /// every version of its tree that declares it, and `memo`'s entry
    /// for a synthesized class side, which is a memo of its tree and is
    /// dropped with it. A holder in another tree is a holder outside, so
    /// that a tree is never freed while another one still holds a version
    /// of it.
    pub(crate) fn held_mixin_ids(
        &self,
        trees: &[Vec<MixinId>],
        memo: &crate::hash::IdentityMap<MixinId, Rc<Mixin>>,
    ) -> BitSet {
        let mut tree_of: Vec<Option<usize>> = vec![None; self.tables.mixins.len()];
        for (tree, members) in trees.iter().enumerate() {
            for id in members {
                tree_of[id.index()] = Some(tree);
            }
        }
        let same_tree = |holder: MixinId, held: MixinId| {
            let tree = |id: MixinId| tree_of.get(id.index()).copied().flatten();
            tree(holder).is_some() && tree(holder) == tree(held)
        };
        // Every version the tables hold, with its id.
        let versions: Vec<(MixinId, &Rc<Mixin>)> = self
            .tables
            .mixins
            .iter()
            .enumerate()
            .filter_map(|(row, entry)| Some((mixin_id_of_row(row), &entry.as_ref()?.current)))
            .chain(
                self.tables
                    .superseded
                    .iter()
                    .filter_map(|version| Some((self.known_mixin_id(version)?, version))),
            )
            .collect();
        let mut structural: crate::hash::IdentityMap<usize, usize> =
            crate::hash::IdentityMap::default();
        for (id, version) in &versions {
            *structural.entry(mixin_key(version)).or_default() += 1;
            if let Some(class_side) = &version.class_side {
                if self
                    .known_mixin_id(class_side)
                    .is_some_and(|class_side_id| same_tree(*id, class_side_id))
                {
                    *structural.entry(mixin_key(class_side)).or_default() += 1;
                }
            }
        }
        for (instance_side, synthesized) in memo {
            if self
                .known_mixin_id(synthesized)
                .is_some_and(|class_side_id| same_tree(*instance_side, class_side_id))
            {
                *structural.entry(mixin_key(synthesized)).or_default() += 1;
            }
        }
        let mut held = BitSet::default();
        for (id, version) in &versions {
            let inside = structural.get(&mixin_key(version)).copied().unwrap_or(0);
            if Rc::strong_count(version) > inside {
                held.insert(mixin_row_number(*id));
            }
        }
        held
    }

    /// Frees the row of every id in `freed`, and drops every superseded
    /// version of one: the index goes to the free rows, and each version's
    /// address leaves `mixin_ids`, so that neither the id nor the address
    /// minted again is ever taken for the dead mixin. Answers the dropped
    /// versions, which the caller lets go of once it has checked that
    /// nothing else holds one. `freed` must be whole trees
    /// (`module_trees`) none of whose versions is held outside the tables
    /// (`held_mixin_ids`).
    pub(crate) fn free_mixins(&mut self, freed: &BitSet) -> Vec<Rc<Mixin>> {
        let mut dropped = Vec::new();
        for row in 0..self.tables.mixins.len() {
            let number = u32::try_from(row).expect("a mixin row fits a u32");
            if !freed.contains(number) {
                continue;
            }
            let Some(entry) = self.tables.mixins[row].take() else {
                continue;
            };
            self.tables.mixin_ids.remove(&mixin_key(&entry.current));
            self.tables.free_mixins.free(number);
            dropped.push(entry.current);
        }
        let Tables {
            superseded,
            mixin_ids,
            ..
        } = &mut self.tables;
        superseded.retain(|version| {
            let key = mixin_key(version);
            let of_a_freed_id = mixin_ids
                .get(&key)
                .is_some_and(|id| freed.contains(mixin_row_number(*id)));
            if of_a_freed_id {
                mixin_ids.remove(&key);
                dropped.push(Rc::clone(version));
            }
            !of_a_freed_id
        });
        dropped
    }

    /// The address of every body that may still run: each one a marked
    /// closure or frame, or a running activation, names by id (`codes`).
    pub(crate) fn running_bodies(&self, codes: &BitSet) -> HashSet<usize> {
        self.tables
            .codes
            .iter()
            .enumerate()
            .filter(|(row, _)| codes.contains(u32::try_from(*row).expect("a code row fits a u32")))
            .filter_map(|(_, entry)| Some(Rc::as_ptr(&entry.as_ref()?.code) as usize))
            .collect()
    }

    /// Drops every superseded version only `superseded` holds and no
    /// running body (`running`) was compiled in, with its `mixin_ids`
    /// entry, until none is left: a superseded instance side holds its
    /// class side, which the next round may drop. A version a closure or
    /// frame still runs code of is kept, because it is what admits that
    /// code into an image of its module (`image::extra_bodies`).
    pub(crate) fn drop_superseded(&mut self, running: &HashSet<usize>) {
        let Tables {
            superseded,
            mixin_ids,
            ..
        } = &mut self.tables;
        // Whether a version runs is asked only of one only the table holds,
        // and once: decoding the bodies of every version kept anyway would
        // cost each full collection for nothing.
        let mut runs: HashMap<usize, bool> = HashMap::new();
        loop {
            let before = superseded.len();
            superseded.retain(|version| {
                if Rc::strong_count(version) > 1 {
                    return true;
                }
                let key = mixin_key(version);
                let still_running = *runs.entry(key).or_insert_with(|| {
                    let mut bodies = HashSet::new();
                    crate::image::version_bodies(version, &mut bodies);
                    !bodies.is_disjoint(running)
                });
                if !still_running {
                    mixin_ids.remove(&key);
                }
                still_running
            });
            if superseded.len() == before {
                break;
            }
        }
    }

    /// Gives back the mixin table's trailing free rows.
    pub(crate) fn truncate_mixin_rows(&mut self) {
        super::ids::truncate_trailing(&mut self.tables.mixins, &mut self.tables.free_mixins);
    }

    /// Frees every code id `codes` does not name: the caller's set holds
    /// those of every marked closure and frame and of every running
    /// activation (`GatheredIds::running_codes`), every holder of one. The
    /// body leaves `code_ids` with its row and lives on with whoever else
    /// holds it; a later push, closure or frame registers it again.
    pub(crate) fn free_unnamed_codes(&mut self, codes: &BitSet) {
        let mut freed = false;
        for row in 0..self.tables.codes.len() {
            let number = u32::try_from(row).expect("a code row fits a u32");
            if codes.contains(number) {
                continue;
            }
            if let Some(entry) = self.tables.codes[row].take() {
                self.tables
                    .code_ids
                    .remove(&(Rc::as_ptr(&entry.code) as usize));
                self.tables.free_codes.free(number);
                freed = true;
            }
        }
        super::ids::truncate_trailing(&mut self.tables.codes, &mut self.tables.free_codes);
        if freed {
            // Every body's cached id may name a freed row now.
            self.code_table_epoch = super::next_code_table_epoch();
        }
    }
}

/// The `BitSet` row of a mixin id.
pub(crate) fn mixin_row_number(id: MixinId) -> u32 {
    u32::try_from(id.index()).expect("a mixin row fits a u32")
}

fn mixin_id_of_row(row: usize) -> MixinId {
    MixinId::from_index(row).expect("a mixin row fits a u32")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(value: i64) -> u64 {
        Value::small_integer(value).word().bits()
    }

    /// A row at or past its table names nothing: gathering it marks no row
    /// and does not size the bit set, so a crafted image cannot make
    /// marking allocate 512 MiB with a field of `u32::MAX`.
    #[test]
    fn a_row_past_its_table_is_not_gathered() {
        let mut gathered = GatheredIds::new(4, 4, 4);
        let huge = number(i64::from(u32::MAX));
        gathered.class_words(huge, huge);
        gathered.closure_words(huge, huge);
        gathered.frame_word(huge);
        gathered.class_words(number(4), number(4));
        assert!(gathered.applications.words.is_empty());
        assert!(gathered.applied_clauses.words.is_empty());
        assert!(gathered.codes.words.is_empty());
        gathered.class_words(number(3), huge);
        gathered.frame_word(number(3));
        assert!(gathered.applications.contains(3));
        assert!(gathered.codes.contains(3));
        assert_eq!(gathered.codes.words.len(), 1);
    }
}
