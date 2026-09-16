//! Inline caches (`docs/design/dispatch.md`, "3b -- The caches"): the table of
//! every call site's entry, per heap, reached from an activation's
//! `feedback_base` -- V8's split of shared code and per-context feedback,
//! reached the way HotSpot's interpreter reaches `MethodData`.

use std::rc::Rc;

use crate::bytecode::CompiledCode;
use crate::hash::IdentityMap;

/// Where an activation's code family starts in its heap's cache table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedbackBase(u32);

impl FeedbackBase {
    /// A closure decoded from an image, or a method step the uncached path
    /// decided: the range is found or assigned when the activation is pushed.
    pub const NOT_ASSIGNED: FeedbackBase = FeedbackBase(u32::MAX);

    /// Only this task's own tests call it: Task 7's site lookup is the
    /// production caller, once a cache entry exists to index with it.
    #[allow(dead_code)]
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

pub(crate) struct InlineCaches {
    /// One slot per call site of every family that has run. Task 7 makes
    /// these `CacheEntry`s; until then the length is all that is kept.
    site_count: usize,
    /// Code identity -> the start of its family's range.
    families: IdentityMap<usize, FeedbackBase>,
    /// Every body in `families`, held so that its address -- the key -- is
    /// never reused by another body while the key is in the map.
    family_codes: Vec<Rc<CompiledCode>>,
}

impl InlineCaches {
    pub(crate) fn new() -> InlineCaches {
        InlineCaches {
            site_count: 0,
            families: IdentityMap::default(),
            family_codes: Vec::new(),
        }
    }

    /// `code`'s range, assigned the first time its family's root is
    /// activated, so that a family that never runs takes no room -- as V8
    /// allocates a feedback vector lazily. A body with no sends is given
    /// base 0 and no room: no site of it is ever indexed.
    pub(crate) fn base_for(&mut self, code: &Rc<CompiledCode>) -> FeedbackBase {
        if code.family_size == 0 {
            return FeedbackBase(0);
        }
        let key = Rc::as_ptr(code) as usize;
        if let Some(base) = self.families.get(&key) {
            return *base;
        }
        let base = self.fresh_range(code.family_size);
        self.families.insert(key, base);
        self.family_codes.push(code.clone());
        base
    }

    /// A new range of `size` sites at the end of the table. `NOT_ASSIGNED`
    /// if the table cannot grow that far, which leaves every site of the
    /// family uncached.
    pub(crate) fn fresh_range(&mut self, size: u32) -> FeedbackBase {
        let start = self.site_count;
        match u32::try_from(start + size as usize) {
            Ok(end) if end < FeedbackBase::NOT_ASSIGNED.0 => {
                self.site_count = end as usize;
                FeedbackBase(start as u32)
            }
            _ => FeedbackBase::NOT_ASSIGNED,
        }
    }

    /// Only this task's own tests call it: Task 8's statistics are the
    /// production caller.
    #[allow(dead_code)]
    pub(crate) fn table_len(&self) -> usize {
        self.site_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{CodeKind, CompiledCode, Instruction};
    use ns_syntax::Span;

    fn body(family_size: u32) -> Rc<CompiledCode> {
        Rc::new(CompiledCode {
            kind: CodeKind::Method,
            instructions: vec![Instruction::Return { register: 0 }],
            argument_count: 0,
            register_count: 1,
            statement_program_counter: vec![],
            family_size,
            source: Span::new(0, 0),
        })
    }

    /// A family gets one contiguous range the first time it is asked for,
    /// and the same one every time after; ranges never overlap.
    #[test]
    fn a_family_keeps_the_range_it_was_first_given() {
        let mut caches = InlineCaches::new();
        let (first, second) = (body(3), body(2));
        let first_base = caches.base_for(&first);
        let second_base = caches.base_for(&second);
        assert_eq!(caches.base_for(&first), first_base);
        assert_eq!(first_base.index(), 0);
        assert_eq!(second_base.index(), 3);
        assert_eq!(caches.table_len(), 5);
    }

    /// A body with no sends needs no range and never allocates one.
    #[test]
    fn a_family_without_sends_takes_no_room() {
        let mut caches = InlineCaches::new();
        let base = caches.base_for(&body(0));
        assert_ne!(base, FeedbackBase::NOT_ASSIGNED);
        assert_eq!(caches.table_len(), 0);
    }

    /// A decoded closure's range is fresh each time it is asked for: any
    /// range of the family's size is a correct one.
    #[test]
    fn a_fresh_range_is_appended() {
        let mut caches = InlineCaches::new();
        caches.base_for(&body(2));
        assert_eq!(caches.fresh_range(4).index(), 2);
        assert_eq!(caches.table_len(), 6);
    }
}
