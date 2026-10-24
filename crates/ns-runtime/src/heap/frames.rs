//! The stack of running activations as frames of tagged words in one
//! reservation (`docs/design/execution.md`, "Representation"). Frames lie
//! back to back, innermost highest; each is a header of
//! `frame_header::COUNT` words followed by its registers, and every word
//! of it is a valid tagged `Word` -- a number a `SmallInteger`, an absent
//! handle nil -- so that the collector can trace `[0, top)` as one linear
//! range of values, with no stack map and no walk of frames.

use super::space::reserve_falling_back;
use super::{out_of_memory, Word};
use ns_memory::{MemoryError, Reservation};

/// How much address space a heap's frame stack reserves. Reserved space
/// costs nothing until it is committed; this bounds the words of every
/// running register file together, a net beneath the depth limit for
/// frames whose register files the activation count does not bound.
#[cfg(target_pointer_width = "64")]
pub(super) const STACK_RESERVATION: usize = 16 << 30;
#[cfg(not(target_pointer_width = "64"))]
pub(super) const STACK_RESERVATION: usize = 256 << 20;

#[cfg(test)]
thread_local! {
    /// The reservation, in bytes, the next `ActivationStack` of this thread
    /// takes instead of `STACK_RESERVATION`: how a test gets a stack small
    /// enough to fill (`with_stack_reservation`).
    static STACK_RESERVATION_OVERRIDE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// Runs `body` with every stack this thread builds reserving `bytes`, for a
/// test of the stack's bound. The override ends with `body`.
#[cfg(test)]
pub(crate) fn with_stack_reservation<T>(bytes: usize, body: impl FnOnce() -> T) -> T {
    STACK_RESERVATION_OVERRIDE.with(|cell| cell.set(Some(bytes)));
    let answer = body();
    STACK_RESERVATION_OVERRIDE.with(|cell| cell.set(None));
    answer
}

/// The reservation a new stack takes.
pub(super) fn stack_reservation() -> usize {
    #[cfg(test)]
    if let Some(bytes) = STACK_RESERVATION_OVERRIDE.with(|cell| cell.get()) {
        return bytes;
    }
    STACK_RESERVATION
}

/// How much of a reservation only the depth limit's reserve may use, as a
/// fraction of it: 1 in this many words. An ordinary push is refused once
/// it would end in the last 1/16th, as one counted push is refused at
/// `MAX_ACTIVATION_DEPTH`; a push made while the depth reserve is held
/// (`Mutator::push`) may use all of it, so that the `signal` of the
/// overflow, its handler search and its handler have room to run.
const RESERVE_FRACTION: usize = 16;

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
    /// How many registers follow the header, a `SmallInteger`: the window
    /// the push wrote, arguments and temporaries together, so that the
    /// frame above starts where they end. A word of its own, since a
    /// factory is handed as many arguments as its send has keywords,
    /// which no field of the flags could bound.
    pub const REGISTER_COUNT: usize = 12;
    /// The activation's generation, a `SmallInteger`; debug builds only.
    #[cfg(debug_assertions)]
    pub const GENERATION: usize = 13;
    /// The header's size: the first register's offset.
    #[cfg(debug_assertions)]
    pub const COUNT: usize = 14;
    /// The header's size: the first register's offset.
    #[cfg(not(debug_assertions))]
    pub const COUNT: usize = 13;
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
/// 8-39, then the two booleans; 42 bits, far inside 61.
const KIND_TAG_MASK: u64 = 0xFF;
const ARGUMENT_COUNT_SHIFT: u32 = 8;
const ARGUMENT_COUNT_MASK: u64 = 0xFFFF_FFFF;
const COUNTED_BIT: u64 = 1 << 40;
const RETAINS_REGISTERS_BIT: u64 = 1 << 41;
/// The bits below a `SmallInteger`'s payload.
const SMALL_INTEGER_TAG_BITS: u32 = 3;
/// A `SmallInteger`'s tag.
const SMALL_INTEGER_TAG: u64 = 0b001;

/// What a frame's flags word holds, packed into one `SmallInteger` as
/// Cog's and Primordial Soup's frame flags are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FrameFlags {
    /// One of `kind_tag`'s constants.
    pub(super) kind_tag: u8,
    /// The arguments copied into the first registers.
    pub(super) argument_count: u32,
    /// Whether the activation counts against the depth limit.
    pub(super) counted: bool,
    /// Whether a frame object married to the activation keeps its
    /// registers when the activation returns.
    pub(super) retains_registers: bool,
}

impl FrameFlags {
    /// The flags as the `SmallInteger` word the header stores.
    #[inline(always)]
    pub(super) fn word(self) -> Word {
        let mut payload =
            u64::from(self.kind_tag) | (u64::from(self.argument_count) << ARGUMENT_COUNT_SHIFT);
        if self.counted {
            payload |= COUNTED_BIT;
        }
        if self.retains_registers {
            payload |= RETAINS_REGISTERS_BIT;
        }
        number_word(payload)
    }

    /// The flags a header's flags word holds; `word` must be one `word`
    /// answered.
    #[inline(always)]
    pub(super) fn of(word: Word) -> FrameFlags {
        let payload = number_of(word);
        FrameFlags {
            kind_tag: (payload & KIND_TAG_MASK) as u8,
            argument_count: ((payload >> ARGUMENT_COUNT_SHIFT) & ARGUMENT_COUNT_MASK) as u32,
            counted: payload & COUNTED_BIT != 0,
            retains_registers: payload & RETAINS_REGISTERS_BIT != 0,
        }
    }
}

/// A header word's number -- a code id, a program counter, an offset, a
/// row, a guard -- as the `SmallInteger` the word holds. Every one a frame
/// stores is far inside 61 bits, so the encoding is exact and needs none
/// of `count_word`'s saturation, which the push would pay for.
#[inline(always)]
pub(super) fn number_word(number: u64) -> Word {
    debug_assert!(
        number < 1 << 60,
        "internal: a frame's number fits a SmallInteger"
    );
    Word::from_bits((number << SMALL_INTEGER_TAG_BITS) | SMALL_INTEGER_TAG)
}

/// The number `number_word` stored; `word` must be one it answered.
#[inline(always)]
pub(super) fn number_of(word: Word) -> u64 {
    debug_assert!(
        word.is_non_negative_small_integer(),
        "internal: a frame's number word is a SmallInteger of 0 or more"
    );
    word.bits() >> SMALL_INTEGER_TAG_BITS
}

/// A push the reservation cannot hold, which the caller raises as the
/// depth limit's `stack overflow: recursion too deep`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReservationExhausted;

/// The frames of the running activations: one reservation of words,
/// committed from its base as the stack deepens and never decommitted
/// while it lives. Everything it lends is `[0, top)`, so the words a
/// truncation left above the top are never read again, and the caller of a
/// push writes every word of the frame it hands out.
pub(super) struct FrameStack {
    reservation: Reservation,
    /// The first word past the innermost frame.
    top: usize,
    /// The end of the committed range, in words: a whole number of pages,
    /// never below `top`.
    committed: usize,
    /// The reservation's size, in words: no frame ends past it.
    limit: usize,
    /// Where an ordinary frame must end by, in words: `limit` less the
    /// reserve. Only a reserved push (`push_reserved`) ends past it.
    ordinary_limit: usize,
    /// Where an ordinary push may end without a growth: the committed end
    /// or `ordinary_limit`, whichever is lower, so that the one comparison
    /// of the hot path also refuses a frame that would use the reserve,
    /// though a reserved push has committed past it.
    ordinary_end: usize,
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
            ordinary_limit: limit - limit / RESERVE_FRACTION,
            ordinary_end: 0,
        })
    }

    /// The first word past the innermost frame.
    pub(super) fn top(&self) -> usize {
        self.top
    }

    /// The most words the stack can ever hold.
    #[cfg(test)]
    pub(super) fn limit(&self) -> usize {
        self.limit
    }

    /// Pushes a frame of `words` words and answers its word offset and its
    /// words; `ReservationExhausted`, the top untouched, when the
    /// reservation cannot hold it. The frame's words are whatever the stack
    /// last held there -- a popped frame's, or zero where nothing was yet --
    /// so the caller writes every one of them before anything reads the
    /// stack again: a word a popped frame left could name an object since
    /// moved or reclaimed. One comparison against the committed end and the
    /// slice's one, with nothing written twice; committing more is the cold
    /// path's.
    #[inline(always)]
    pub(super) fn push(
        &mut self,
        words: usize,
    ) -> Result<(usize, &mut [u64]), ReservationExhausted> {
        let start = self.top;
        let end = start + words;
        if end > self.ordinary_end {
            self.grow(words, self.ordinary_limit)?;
        }
        self.top = end;
        Ok((start, &mut self.reservation.accessible_mut()[start..end]))
    }

    /// `push`, but a frame may end anywhere in the reservation, the reserve
    /// included: the push of an activation the depth limit's reserve is
    /// held for. Cold, since only the unwinding of an overflow makes it.
    #[cold]
    #[inline(never)]
    pub(super) fn push_reserved(
        &mut self,
        words: usize,
    ) -> Result<(usize, &mut [u64]), ReservationExhausted> {
        let start = self.top;
        let end = start + words;
        if end > self.committed {
            self.grow(words, self.limit)?;
        }
        self.top = end;
        Ok((start, &mut self.reservation.accessible_mut()[start..end]))
    }

    /// Whether the top is within the reserve of the end of the ordinary
    /// frames' room: the stack is as full as an error should find it for
    /// its `signal` to take the depth limit's reserve, as a counted depth
    /// within `DEPTH_RESERVE` of the limit is.
    pub(super) fn within_the_reserve_of_the_limit(&self) -> bool {
        self.top + (self.limit - self.ordinary_limit) >= self.ordinary_limit
    }

    /// Commits whole pages from the committed end up to at least
    /// `top + words`: double what is committed, at least `MINIMUM_GROWTH`
    /// and at most `MAXIMUM_GROWTH` more, within the reservation, or just
    /// what the push needs when the operating system refuses that much.
    /// Refuses, committing nothing, past `cap` words. A commit the
    /// operating system refuses outright is running out of memory, as the
    /// object space's is.
    #[cold]
    #[inline(never)]
    fn grow(&mut self, words: usize, cap: usize) -> Result<(), ReservationExhausted> {
        let needed = match self.top.checked_add(words) {
            Some(end) if end <= cap => end,
            _ => return Err(ReservationExhausted),
        };
        let page = Reservation::page_size();
        let size = self.reservation.size();
        let committed_bytes = self.committed * 8;
        // The reservation's size is a whole number of pages, so rounding an
        // end inside it up to a page stays inside it.
        let needed_bytes = (needed * 8).div_ceil(page) * page;
        let growth = committed_bytes.clamp(MINIMUM_GROWTH, MAXIMUM_GROWTH);
        let generous = (committed_bytes + growth)
            .min(cap * 8)
            .div_ceil(page)
            .saturating_mul(page)
            .min(size);
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
        self.committed = self.committed.max(end / 8);
        self.ordinary_end = self.committed.min(self.ordinary_limit);
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

    /// The words of every frame, `[0, top)`: one comparison, of the top
    /// against the committed end it never passes.
    #[inline(always)]
    pub(super) fn words(&self) -> &[u64] {
        &self.reservation.accessible()[..self.top]
    }

    /// As `words`, mutably.
    #[inline(always)]
    pub(super) fn words_mut(&mut self) -> &mut [u64] {
        &mut self.reservation.accessible_mut()[..self.top]
    }

    /// Word `index` of a live frame. The register access of the run loop,
    /// so it makes one bounds check, against the committed end; that
    /// `index` lies below the top -- inside a live frame's window -- is the
    /// caller's, which a debug build checks here and the heap's
    /// `register_slot` against the frame's own window. A release build
    /// that broke it would read a stale word of this stack, never another
    /// object's memory.
    #[inline(always)]
    pub(super) fn word(&self, index: usize) -> u64 {
        debug_assert!(
            index < self.top,
            "internal: a stack word lies below the top"
        );
        self.reservation.accessible()[index]
    }

    /// Writes word `index` of a live frame, as `word` reads it.
    #[inline(always)]
    pub(super) fn set_word(&mut self, index: usize, bits: u64) {
        debug_assert!(
            index < self.top,
            "internal: a stack word lies below the top"
        );
        self.reservation.accessible_mut()[index] = bits;
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
            for argument_count in [0, 1, u32::MAX] {
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
            frame_header::REGISTER_COUNT,
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
    fn a_push_past_the_committed_end_commits_the_words_it_answers() {
        let mut stack = FrameStack::new(1 << 20).expect("reserve");
        let frame_words = usize::from(u16::MAX) + frame_header::COUNT;
        assert!(frame_words * 8 > Reservation::page_size());
        let before = stack.top();
        let (offset, _) = stack.push(frame_words).expect("push");
        assert_eq!(offset, before);
        assert_eq!(stack.top(), before + frame_words);
        stack.words_mut()[offset..].fill(Word::NIL.bits());
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
                Ok((offset, frame)) => {
                    assert_eq!(offset, before);
                    assert_eq!(frame.len(), frame_words);
                    pushed += 1;
                }
                Err(ReservationExhausted) => {
                    assert_eq!(stack.top(), before);
                    assert!(before + frame_words > stack.ordinary_limit);
                    break;
                }
            }
        }
        assert!(pushed > 0);
        assert_eq!(stack.words().len(), stack.top());
    }

    #[test]
    fn a_reserved_push_may_use_the_room_an_ordinary_push_leaves() {
        let mut stack = FrameStack::new(64 << 10).expect("reserve");
        let frame_words = frame_header::COUNT + 100;
        while stack.push(frame_words).is_ok() {}
        let ordinary_top = stack.top();
        assert!(
            stack.limit() - ordinary_top >= frame_words,
            "a reserve is left"
        );
        let mut reserved = 0;
        loop {
            let before = stack.top();
            match stack.push_reserved(frame_words) {
                Ok((offset, _)) => {
                    assert_eq!(offset, before);
                    reserved += 1;
                }
                Err(ReservationExhausted) => {
                    assert_eq!(stack.top(), before);
                    assert!(before + frame_words > stack.limit());
                    break;
                }
            }
        }
        assert!(reserved > 0);
        // Back below the ordinary room, an ordinary push is refused only at
        // the ordinary limit again, though the reserve has been committed.
        stack.truncate(ordinary_top);
        assert!(stack.push(frame_words).is_err());
        stack.truncate(ordinary_top - frame_words);
        assert!(stack.push(frame_words).is_ok());
    }

    #[test]
    fn truncate_then_push_reuses_the_offset() {
        let mut stack = FrameStack::new(1 << 20).expect("reserve");
        let frame_words = frame_header::COUNT + 4;
        stack.push(frame_words).expect("push");
        let (offset, _) = stack.push(frame_words).expect("push");
        for word in &mut stack.words_mut()[offset..] {
            *word = number_word(7).bits();
        }
        stack.truncate(offset);
        assert_eq!(stack.top(), offset);
        assert_eq!(stack.words().len(), offset);
        let (again, _) = stack.push(frame_words).expect("push");
        assert_eq!(again, offset);
        assert_eq!(stack.words().len(), offset + frame_words);
    }
}
