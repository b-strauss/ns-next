//! Which byte ranges of a reservation are committed, and how they are
//! protected. The operating system knows this too, but asking it per access
//! would cost a system call; the reservation asks this map before it lends a
//! slice, so a slice can never cover a page that faults.

use std::ops::Range;

use crate::Protection;

/// Disjoint, sorted byte ranges with their protection; a byte in no range is
/// not committed. Adjacent ranges of one protection are merged, so a range
/// spanning both is found as one.
#[derive(Debug, Default)]
pub(crate) struct PageMap {
    ranges: Vec<(Range<usize>, Protection)>,
}

impl PageMap {
    pub(crate) fn new() -> PageMap {
        PageMap { ranges: Vec::new() }
    }

    /// Records `range` as committed with `state`, or as not committed for
    /// `None`, overriding whatever it was.
    pub(crate) fn set(&mut self, range: Range<usize>, state: Option<Protection>) {
        if range.is_empty() {
            return;
        }
        let mut kept = Vec::with_capacity(self.ranges.len() + 2);
        for (existing, protection) in self.ranges.drain(..) {
            if existing.end <= range.start || existing.start >= range.end {
                kept.push((existing, protection));
                continue;
            }
            if existing.start < range.start {
                kept.push((existing.start..range.start, protection));
            }
            if existing.end > range.end {
                kept.push((range.end..existing.end, protection));
            }
        }
        if let Some(protection) = state {
            kept.push((range, protection));
        }
        kept.sort_by_key(|(existing, _)| existing.start);
        let mut merged: Vec<(Range<usize>, Protection)> = Vec::with_capacity(kept.len());
        for (existing, protection) in kept {
            if let Some((last, last_protection)) = merged.last_mut()
                && last.end == existing.start
                && *last_protection == protection
            {
                last.end = existing.end;
                continue;
            }
            merged.push((existing, protection));
        }
        self.ranges = merged;
    }

    /// The end of the read-write range that starts at byte 0, or 0 when byte 0
    /// is not committed read-write. Adjacent ranges of one protection are
    /// merged, so the first range is the whole prefix.
    pub(crate) fn accessible_prefix(&self) -> usize {
        match self.ranges.first() {
            Some((range, Protection::ReadWrite)) if range.start == 0 => range.end,
            _ => 0,
        }
    }

    /// Whether every byte of `range` is committed, whatever its protection.
    pub(crate) fn is_committed(&self, range: Range<usize>) -> bool {
        self.covered(range, |_| true)
    }

    /// Whether every byte of `range` is committed read-write.
    pub(crate) fn is_accessible(&self, range: Range<usize>) -> bool {
        self.covered(range, |protection| protection == Protection::ReadWrite)
    }

    fn covered(&self, range: Range<usize>, admits: impl Fn(Protection) -> bool) -> bool {
        if range.is_empty() {
            return false;
        }
        let mut position = range.start;
        for (existing, protection) in &self.ranges {
            if existing.end <= position {
                continue;
            }
            if existing.start > position || !admits(*protection) {
                return false;
            }
            position = existing.end;
            if position >= range.end {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_map_has_nothing_committed() {
        let map = PageMap::new();
        assert!(!map.is_committed(0..4096));
        assert!(!map.is_accessible(0..4096));
    }

    #[test]
    fn a_committed_range_is_accessible_and_its_neighbours_are_not() {
        let mut map = PageMap::new();
        map.set(4096..8192, Some(Protection::ReadWrite));
        assert!(map.is_accessible(4096..8192));
        assert!(map.is_accessible(5000..6000));
        assert!(!map.is_accessible(0..4096));
        assert!(!map.is_accessible(4000..5000));
        assert!(!map.is_accessible(8000..9000));
    }

    #[test]
    fn a_no_access_range_is_committed_but_not_accessible() {
        let mut map = PageMap::new();
        map.set(0..8192, Some(Protection::ReadWrite));
        map.set(4096..8192, Some(Protection::NoAccess));
        assert!(map.is_committed(0..8192));
        assert!(map.is_accessible(0..4096));
        assert!(!map.is_accessible(0..8192));
        assert!(!map.is_accessible(4096..5000));
    }

    #[test]
    fn decommitting_the_middle_splits_a_range() {
        let mut map = PageMap::new();
        map.set(0..12288, Some(Protection::ReadWrite));
        map.set(4096..8192, None);
        assert!(map.is_accessible(0..4096));
        assert!(!map.is_committed(4096..8192));
        assert!(map.is_accessible(8192..12288));
        assert!(!map.is_committed(0..12288));
    }

    #[test]
    fn adjacent_ranges_of_one_protection_cover_a_range_spanning_both() {
        let mut map = PageMap::new();
        map.set(0..4096, Some(Protection::ReadWrite));
        map.set(4096..8192, Some(Protection::ReadWrite));
        assert!(map.is_accessible(0..8192));
    }

    #[test]
    fn the_accessible_prefix_is_the_read_write_range_at_the_base() {
        let mut map = PageMap::new();
        assert_eq!(map.accessible_prefix(), 0);
        map.set(4096..8192, Some(Protection::ReadWrite));
        assert_eq!(
            map.accessible_prefix(),
            0,
            "a range not at the base is no prefix"
        );
        map.set(0..4096, Some(Protection::ReadWrite));
        assert_eq!(
            map.accessible_prefix(),
            8192,
            "adjacent ranges merge into one prefix"
        );
        map.set(4096..8192, Some(Protection::NoAccess));
        assert_eq!(map.accessible_prefix(), 4096);
        map.set(0..4096, None);
        assert_eq!(map.accessible_prefix(), 0);
    }

    #[test]
    fn an_empty_range_is_never_accessible() {
        let mut map = PageMap::new();
        map.set(0..4096, Some(Protection::ReadWrite));
        assert!(!map.is_accessible(100..100));
        assert!(!map.is_committed(100..100));
    }
}
