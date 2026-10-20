//! The allocators of the per-heap ids that a full collection gives back:
//! the free rows of a table and the guard ids.

use crate::mixin::DispatchId;
use std::collections::BTreeSet;

/// The free rows of one table: minted smallest first, so that a table
/// keeps its rows dense and can give its trailing ones back.
#[derive(Default)]
pub(crate) struct FreeIndices {
    free: BTreeSet<u32>,
}

impl FreeIndices {
    /// The smallest free index, taken out of the set; `None` when none is
    /// free.
    pub(crate) fn take_smallest(&mut self) -> Option<u32> {
        self.free.pop_first()
    }

    /// Makes `index` free. A compiler-invariant violation panics: freeing
    /// an index twice.
    pub(crate) fn free(&mut self, index: u32) {
        let inserted = self.free.insert(index);
        assert!(inserted, "internal: index {index} freed twice");
    }

    // Freed by the collection's reclamation of dead rows.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_free(&self, index: u32) -> bool {
        self.free.contains(&index)
    }

    // Freed by the collection's reclamation of dead rows.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn len(&self) -> usize {
        self.free.len()
    }
}

/// Pops `rows`' trailing `None`s and forgets their indices in `free`, so a
/// table whose last rows died gives their memory back.
pub(crate) fn truncate_trailing<T>(rows: &mut Vec<Option<T>>, free: &mut FreeIndices) {
    while matches!(rows.last(), Some(None)) {
        rows.pop();
        let index = u32::try_from(rows.len()).expect("a table row fits a u32");
        let was_free = free.free.remove(&index);
        debug_assert!(was_free, "internal: a trailing None row is a free index");
    }
}

/// The guard ids: the smallest freed one first, else the next never
/// minted, below `DispatchId::LIMIT`.
pub(crate) struct DispatchIdAllocator {
    next: u32,
    free: FreeIndices,
}

impl DispatchIdAllocator {
    pub(crate) fn new() -> DispatchIdAllocator {
        DispatchIdAllocator {
            next: DispatchId::FIRST_MINTED,
            free: FreeIndices::default(),
        }
    }

    /// `None` once every id below `LIMIT` is in use.
    pub(crate) fn mint(&mut self) -> Option<DispatchId> {
        if let Some(index) = self.free.take_smallest() {
            return Some(DispatchId::from_counter(index));
        }
        if self.next >= DispatchId::LIMIT {
            return None;
        }
        let id = DispatchId::from_counter(self.next);
        self.next += 1;
        Some(id)
    }

    /// Gives `id` back. `NONE`, `VACANT` and `FORWARDED` are never minted
    /// and never freed.
    pub(crate) fn free(&mut self, id: DispatchId) {
        debug_assert!(id.bits() >= DispatchId::FIRST_MINTED && id.bits() < self.next);
        self.free.free(id.bits());
    }

    /// One past the largest id ever minted, for the tests that bound
    /// growth.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn high_water(&self) -> u32 {
        self.next
    }

    /// For the exhaustion tests: the next never-minted id is `next`.
    #[cfg(test)]
    pub(crate) fn set_next_for_test(&mut self, next: u32) {
        self.next = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_smallest_free_index_is_taken_first() {
        let mut free = FreeIndices::default();
        free.free(7);
        free.free(2);
        free.free(5);
        assert_eq!(free.take_smallest(), Some(2));
        assert_eq!(free.take_smallest(), Some(5));
        assert_eq!(free.take_smallest(), Some(7));
        assert_eq!(free.take_smallest(), None);
    }

    #[test]
    fn trailing_free_rows_are_truncated_and_forgotten() {
        let mut rows = vec![Some('a'), None, Some('c'), None, None];
        let mut free = FreeIndices::default();
        for index in [1, 3, 4] {
            free.free(index);
        }
        truncate_trailing(&mut rows, &mut free);
        assert_eq!(rows, vec![Some('a'), None, Some('c')]);
        assert!(free.is_free(1));
        assert!(!free.is_free(3) && !free.is_free(4));
        assert_eq!(free.len(), 1);
    }

    #[test]
    fn a_freed_guard_is_minted_again_before_a_new_one() {
        let mut allocator = DispatchIdAllocator::new();
        let first = allocator.mint().expect("an id");
        let second = allocator.mint().expect("an id");
        allocator.free(first);
        assert_eq!(allocator.mint(), Some(first));
        assert_eq!(allocator.high_water(), second.bits() + 1);
    }

    #[test]
    fn the_allocator_is_exhausted_below_twenty_two_bits() {
        let mut allocator = DispatchIdAllocator::new();
        allocator.set_next_for_test(DispatchId::LIMIT - 1);
        assert_eq!(
            allocator.mint(),
            Some(DispatchId::from_counter(DispatchId::LIMIT - 1))
        );
        assert_eq!(allocator.mint(), None);
        allocator.free(DispatchId::from_counter(DispatchId::LIMIT - 1));
        assert_eq!(
            allocator.mint(),
            Some(DispatchId::from_counter(DispatchId::LIMIT - 1))
        );
    }
}
