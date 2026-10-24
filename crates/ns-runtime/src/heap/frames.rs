//! The stack of running activations as frames of tagged words in one
//! reservation (`docs/design/execution.md`, "Planned: frames as words in
//! one reservation"). Frames lie back to back, innermost highest; each is
//! a header of `frame_header::COUNT` words followed by its registers, and
//! every word of it is a valid tagged `Word` -- a number a `SmallInteger`,
//! an absent handle nil -- so that the collector can trace `[0, top)` as
//! one linear range of values, with no stack map and no walk of frames.

use super::space::reserve_falling_back;
use super::{count_word, out_of_memory, Word};
use ns_memory::{MemoryError, Reservation};

/// How much address space a heap's frame stack reserves. Reserved space
/// costs nothing until it is committed; this bounds the words of every
/// running register file together, a net beneath the depth limit for
/// frames whose register files the activation count does not bound.
#[cfg(target_pointer_width = "64")]
pub(super) const STACK_RESERVATION: usize = 16 << 30;
#[cfg(not(target_pointer_width = "64"))]
pub(super) const STACK_RESERVATION: usize = 256 << 20;

/// The least a growth commits beyond what is committed, in bytes, so that
/// a shallow program commits once.
const MINIMUM_GROWTH: usize = 64 << 10;

/// The most a growth commits beyond what is committed, in bytes, unless
/// the push needs more: growth doubles the committed range up to this
/// step, so that a deep recursion makes one system call per doubling
/// rather than one per page, and a very deep one commits little past what
/// it uses.
const MAXIMUM_GROWTH: usize = 16 << 20;

/// The word offsets of a frame's header from the frame's start; its
/// registers start at `COUNT`. Every word holds a tagged value
/// (`docs/design/execution.md`, the table of header words).
pub(super) mod frame_header {
    /// The receiver, a value.
    pub const RECEIVER: usize = 0;
    /// The class object, or nil for a top-level activation.
    pub const CURRENT_CLASS: usize = 1;
    /// The closure object of a block frame, the class of a class-carrying
    /// kind, the selector as a `SmallInteger` of a method frame, nil
    /// otherwise.
    pub const OPERAND: usize = 2;
    /// The married frame object, or nil.
    pub const REIFIED: usize = 3;
    /// The `FrameFlags`, a `SmallInteger`.
    pub const FLAGS: usize = 4;
    /// The `CodeId` of the body the frame runs, a `SmallInteger`.
    pub const CODE: usize = 5;
    /// The program counter, a `SmallInteger`, stored at the observable
    /// points only.
    pub const PROGRAM_COUNTER: usize = 6;
    /// The feedback base, a `SmallInteger`.
    pub const FEEDBACK_BASE: usize = 7;
    /// The word offset of the frame below, a `SmallInteger`; nil in the
    /// bottom frame.
    pub const CALLER: usize = 8;
    /// Nil for `Deliver`, otherwise the continuation's row in the heap's
    /// side table, a `SmallInteger`.
    pub const CONTINUATION: usize = 9;
    /// The recorded guard, a `SmallInteger`.
    pub const GUARD: usize = 10;
    /// The entered level, a `SmallInteger`.
    pub const ENTERED_LEVEL: usize = 11;
    /// The activation's generation, a `SmallInteger`; debug builds only.
    #[cfg(debug_assertions)]
    pub const GENERATION: usize = 12;
    /// The header's size: the first register's offset.
    #[cfg(debug_assertions)]
    pub const COUNT: usize = 13;
    /// The header's size: the first register's offset.
    #[cfg(not(debug_assertions))]
    pub const COUNT: usize = 12;
}

/// The kind of activation a frame runs, as `FrameFlags` records it: one
/// tag for each `ActivationKind`, an initializer's once for each
/// `InitializerBody`, so that the flags and the operand word together
/// rebuild the kind. A numbering of this module's own, never written to
/// an image: the image's wire tags are another, pinned namespace.
pub(super) mod kind_tag {
    /// `ActivationKind::Method`; the operand holds the selector.
    pub const METHOD: u8 = 0;
    /// `ActivationKind::Closure`; the operand holds the closure.
    pub const CLOSURE: u8 = 1;
    /// `ActivationKind::TopLevel`; the operand is nil.
    pub const TOP_LEVEL: u8 = 2;
    /// `ActivationKind::Initializer` with `InitializerBody::Full`; the
    /// operand holds the class.
    pub const INITIALIZER_FULL: u8 = 3;
    /// `ActivationKind::Initializer` with `InitializerBody::OwnPart`; the
    /// operand holds the class.
    pub const INITIALIZER_OWN_PART: u8 = 4;
    /// `ActivationKind::Initializer` with `InitializerBody::Standalone`;
    /// the operand holds the class.
    pub const INITIALIZER_STANDALONE: u8 = 5;
    /// `ActivationKind::SuperclassClause`; the operand holds the class.
    pub const SUPERCLASS_CLAUSE: u8 = 6;
    /// `ActivationKind::Factory`; the operand holds the class.
    pub const FACTORY: u8 = 7;
}

/// Where each field of `FrameFlags` sits in the flags word's
/// `SmallInteger`: the kind tag in bits 0-7, the argument count in bits
/// 8-23, then the two booleans; 26 bits, far inside 61.
const ARGUMENT_COUNT_SHIFT: u32 = 8;
const COUNTED_BIT: u64 = 1 << 24;
const RETAINS_REGISTERS_BIT: u64 = 1 << 25;
/// The bits below a `SmallInteger`'s payload.
const SMALL_INTEGER_TAG_BITS: u32 = 3;

/// What a frame's flags word holds, packed into one `SmallInteger` as
/// Cog's and Primordial Soup's frame flags are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FrameFlags {
    /// One of `kind_tag`'s constants.
    pub(super) kind_tag: u8,
    /// The arguments copied into the first registers.
    pub(super) argument_count: u16,
    /// Whether the activation counts against the depth limit.
    pub(super) counted: bool,
    /// Whether a frame object married to the activation keeps its
    /// registers when the activation returns.
    pub(super) retains_registers: bool,
}

impl FrameFlags {
    /// The flags as the `SmallInteger` word the header stores.
    pub(super) fn word(self) -> Word {
        let mut payload =
            u64::from(self.kind_tag) | (u64::from(self.argument_count) << ARGUMENT_COUNT_SHIFT);
        if self.counted {
            payload |= COUNTED_BIT;
        }
        if self.retains_registers {
            payload |= RETAINS_REGISTERS_BIT;
        }
        // 26 bits: the conversion is exact on every target, and
        // `count_word` never saturates it.
        count_word(payload as usize)
    }

    /// The flags a header's flags word holds; `word` must be one `word`
    /// answered.
    pub(super) fn of(word: Word) -> FrameFlags {
        debug_assert!(
            word.is_small_integer(),
            "internal: a frame's flags word is a SmallInteger"
        );
        let payload = word.bits() >> SMALL_INTEGER_TAG_BITS;
        FrameFlags {
            kind_tag: (payload & 0xFF) as u8,
            argument_count: ((payload >> ARGUMENT_COUNT_SHIFT) & 0xFFFF) as u16,
            counted: payload & COUNTED_BIT != 0,
            retains_registers: payload & RETAINS_REGISTERS_BIT != 0,
        }
    }
}

/// A push the reservation cannot hold, which the caller raises as the
/// depth limit's `stack overflow: recursion too deep`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StackExhausted;

/// The frames of the running activations: one reservation of words,
/// committed from its base as the stack deepens and never decommitted
/// while it lives. Everything it lends is `[0, top)`, so the words a
/// truncation left above the top are never read again, and a push
/// nil-fills the frame it hands out.
pub(super) struct FrameStack {
    reservation: Reservation,
    /// The first word past the innermost frame.
    top: usize,
    /// The end of the committed range, in words: a whole number of pages,
    /// never below `top`.
    committed: usize,
    /// The reservation's size, in words: no frame ends past it.
    limit: usize,
}

impl FrameStack {
    /// Reserves `reservation_bytes` of address space -- less when the
    /// operating system refuses that much, falling back as the object
    /// space does -- committing none of it.
    pub(super) fn new(reservation_bytes: usize) -> Result<FrameStack, MemoryError> {
        let reservation = reserve_falling_back(reservation_bytes, Reservation::reserve)?;
        let limit = reservation.size() / 8;
        Ok(FrameStack {
            reservation,
            top: 0,
            committed: 0,
            limit,
        })
    }

    /// The first word past the innermost frame.
    pub(super) fn top(&self) -> usize {
        self.top
    }

    /// The most words the stack can ever hold.
    pub(super) fn limit(&self) -> usize {
        self.limit
    }

    /// Pushes a frame of `words` words, every one nil, and answers its word
    /// offset; `StackExhausted`, the top and every word untouched, when the
    /// reservation cannot hold it. One comparison against the committed
    /// end; committing more is the cold path's.
    #[inline]
    pub(super) fn push(&mut self, words: usize) -> Result<usize, StackExhausted> {
        let start = self.top;
        if words > self.committed - start {
            self.grow(words)?;
        }
        let end = start + words;
        match self.reservation.prefix_mut(end * 8) {
            Some(prefix) => prefix[start..].fill(Word::NIL.bits()),
            None => {
                unreachable!("internal: the frame stack's top lies inside its committed prefix")
            }
        }
        self.top = end;
        Ok(start)
    }

    /// Commits whole pages from the committed end up to at least
    /// `top + words`: double what is committed, at least `MINIMUM_GROWTH`
    /// and at most `MAXIMUM_GROWTH` more, within the reservation, or just
    /// what the push needs when the operating system refuses that much.
    /// Refuses, committing nothing, past the reservation. A commit the
    /// operating system refuses outright is running out of memory, as the
    /// object space's is.
    #[cold]
    #[inline(never)]
    fn grow(&mut self, words: usize) -> Result<(), StackExhausted> {
        let needed = match self.top.checked_add(words) {
            Some(end) if end <= self.limit => end,
            _ => return Err(StackExhausted),
        };
        let page = Reservation::page_size();
        let size = self.reservation.size();
        let committed_bytes = self.committed * 8;
        // The reservation's size is a whole number of pages, so rounding an
        // end inside it up to a page stays inside it.
        let needed_bytes = (needed * 8).div_ceil(page) * page;
        let growth = committed_bytes.clamp(MINIMUM_GROWTH, MAXIMUM_GROWTH);
        let generous = (committed_bytes + growth).min(size).div_ceil(page) * page;
        let target = needed_bytes.max(generous);
        let end = if self.reservation.commit(committed_bytes..target).is_ok() {
            target
        } else if self
            .reservation
            .commit(committed_bytes..needed_bytes)
            .is_ok()
        {
            needed_bytes
        } else {
            out_of_memory(needed_bytes - committed_bytes)
        };
        self.committed = end / 8;
        Ok(())
    }

    /// Pops every frame at and above `offset`, which must not lie above the
    /// top. The words above the new top keep whatever they held, but
    /// nothing reads them: `words` lends only below the top, and a push
    /// overwrites them.
    pub(super) fn truncate(&mut self, offset: usize) {
        debug_assert!(
            offset <= self.top,
            "internal: a truncation never raises the top"
        );
        self.top = offset.min(self.top);
    }

    /// The words of every frame, `[0, top)`.
    pub(super) fn words(&self) -> &[u64] {
        match self.reservation.prefix(self.top * 8) {
            Some(words) => words,
            None => {
                unreachable!("internal: the frame stack's top lies inside its committed prefix")
            }
        }
    }

    /// As `words`, mutably.
    pub(super) fn words_mut(&mut self) -> &mut [u64] {
        match self.reservation.prefix_mut(self.top * 8) {
            Some(words) => words,
            None => {
                unreachable!("internal: the frame stack's top lies inside its committed prefix")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_KIND_TAGS: [u8; 8] = [
        kind_tag::METHOD,
        kind_tag::CLOSURE,
        kind_tag::TOP_LEVEL,
        kind_tag::INITIALIZER_FULL,
        kind_tag::INITIALIZER_OWN_PART,
        kind_tag::INITIALIZER_STANDALONE,
        kind_tag::SUPERCLASS_CLAUSE,
        kind_tag::FACTORY,
    ];

    fn assert_all_nil(words: &[u64]) {
        assert!(words.iter().all(|&word| word == Word::NIL.bits()));
    }

    #[test]
    fn flags_round_trip_through_a_small_integer_word() {
        for kind_tag in ALL_KIND_TAGS {
            for argument_count in [0, 1, u16::MAX] {
                for counted in [false, true] {
                    for retains_registers in [false, true] {
                        let flags = FrameFlags {
                            kind_tag,
                            argument_count,
                            counted,
                            retains_registers,
                        };
                        let word = flags.word();
                        assert!(word.is_small_integer());
                        assert_eq!(FrameFlags::of(word), flags);
                    }
                }
            }
        }
    }

    #[test]
    fn the_header_words_are_each_offset_below_count_once() {
        let mut offsets = vec![
            frame_header::RECEIVER,
            frame_header::CURRENT_CLASS,
            frame_header::OPERAND,
            frame_header::REIFIED,
            frame_header::FLAGS,
            frame_header::CODE,
            frame_header::PROGRAM_COUNTER,
            frame_header::FEEDBACK_BASE,
            frame_header::CALLER,
            frame_header::CONTINUATION,
            frame_header::GUARD,
            frame_header::ENTERED_LEVEL,
        ];
        #[cfg(debug_assertions)]
        offsets.push(frame_header::GENERATION);
        offsets.sort_unstable();
        assert_eq!(offsets, (0..frame_header::COUNT).collect::<Vec<_>>());
    }

    #[test]
    fn the_stack_reservation_is_reserved_and_commits_nothing_until_a_push() {
        let stack = FrameStack::new(STACK_RESERVATION).expect("reserve");
        assert!(stack.limit() > 0);
        assert_eq!(stack.top(), 0);
        assert!(stack.words().is_empty());
    }

    #[test]
    fn a_push_past_the_committed_end_commits_and_answers_nil_words() {
        let mut stack = FrameStack::new(1 << 20).expect("reserve");
        let frame_words = usize::from(u16::MAX) + frame_header::COUNT;
        assert!(frame_words * 8 > Reservation::page_size());
        let before = stack.top();
        let offset = stack.push(frame_words).expect("push");
        assert_eq!(offset, before);
        assert_eq!(stack.top(), before + frame_words);
        assert_all_nil(&stack.words()[offset..]);
    }

    #[test]
    fn a_push_the_reservation_cannot_hold_is_refused_and_leaves_the_top() {
        let mut stack = FrameStack::new(64 << 10).expect("reserve");
        let frame_words = frame_header::COUNT + 100;
        let mut pushed = 0;
        loop {
            let before = stack.top();
            match stack.push(frame_words) {
                Ok(offset) => {
                    assert_eq!(offset, before);
                    pushed += 1;
                }
                Err(StackExhausted) => {
                    assert_eq!(stack.top(), before);
                    assert!(before + frame_words > stack.limit());
                    break;
                }
            }
        }
        assert!(pushed > 0);
        assert_eq!(stack.words().len(), stack.top());
    }

    #[test]
    fn truncate_then_push_reuses_the_offset() {
        let mut stack = FrameStack::new(1 << 20).expect("reserve");
        let frame_words = frame_header::COUNT + 4;
        stack.push(frame_words).expect("push");
        let offset = stack.push(frame_words).expect("push");
        for word in &mut stack.words_mut()[offset..] {
            *word = count_word(7).bits();
        }
        stack.truncate(offset);
        assert_eq!(stack.top(), offset);
        assert_eq!(stack.words().len(), offset);
        let again = stack.push(frame_words).expect("push");
        assert_eq!(again, offset);
        assert_all_nil(&stack.words()[offset..]);
    }
}
