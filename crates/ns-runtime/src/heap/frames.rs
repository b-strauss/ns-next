//! The stack of running activations as frames of tagged words in one
//! reservation (`docs/design/execution.md`, "Representation"). Each frame
//! is its arguments, a header of `frame_header::COUNT` words, then the rest
//! of its registers, innermost highest; a callee a send pushes lies on its
//! sender's argument slots, which are its own arguments, every other frame
//! above its sender's whole window. Every word is a valid tagged `Word` --
//! a number a `SmallInteger`, an absent handle nil -- so that the collector
//! can trace `[0, top)` as one linear range of values, with no stack map
//! and no walk of frames.

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

/// The most words a stack may hold: a frame's offset is stored in 32 bits
/// -- an `ActivationReference`, a `RegisterBase`, a caller word read back
/// -- and `u32::MAX` is the empty stack's `NO_FRAME`, so a larger
/// reservation would truncate an offset into another frame's at exit
/// status 0.
const MAXIMUM_STACK_WORDS: usize = 1 << 31;

const _: () = assert!(
    STACK_RESERVATION / 8 <= MAXIMUM_STACK_WORDS,
    "the stack's reservation must keep every frame offset inside 32 bits"
);

#[cfg(test)]
thread_local! {
    /// The reservation, in bytes, the next `ActivationStack` of this thread
    /// takes instead of `STACK_RESERVATION`: how a test gets a stack small
    /// enough to fill (`with_stack_reservation`).
    static STACK_RESERVATION_OVERRIDE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// Runs `body` with every stack this thread builds reserving `bytes`, for a
/// test of the stack's bound. The override ends with `body`, a panic in it
/// included, so that no later test on the thread inherits the small stack.
#[cfg(test)]
pub(crate) fn with_stack_reservation<T>(bytes: usize, body: impl FnOnce() -> T) -> T {
    struct ResetOverride;
    impl Drop for ResetOverride {
        fn drop(&mut self) {
            STACK_RESERVATION_OVERRIDE.with(|cell| cell.set(None));
        }
    }
    STACK_RESERVATION_OVERRIDE.with(|cell| cell.set(Some(bytes)));
    let _reset = ResetOverride;
    body()
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

/// The word offsets of a frame's header from the header's start, which a
/// frame's arguments precede and the rest of its registers follow from
/// `COUNT` on. Every word holds a tagged value (`docs/design/execution.md`,
/// the table of header words).
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
    /// How many registers follow the header, a `SmallInteger`: the frame's
    /// window less its arguments, which lie below the header -- copied
    /// values, temporaries and scratch -- so that the window ends at
    /// `offset + COUNT + REGISTER_COUNT`. A word of its own, since a body's
    /// registers can pass the 16 bits the flags give the argument count.
    pub const REGISTER_COUNT: usize = 12;
    /// The header's size, and how many register numbers it takes between a
    /// frame's arguments and the rest of its registers
    /// (`bytecode::HEADER_REGISTERS`). The same in every build, a debug
    /// build's generation riding in the flags word, so that the register
    /// numbering the compiler decides around the header holds in both.
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
/// 8-23, `counted` at 24, `retains_registers` at 25 and, in a debug build,
/// the 31-bit generation in bits 26-56; 57 bits, inside 61.
const KIND_TAG_MASK: u64 = 0xFF;
const ARGUMENT_COUNT_SHIFT: u32 = 8;
const ARGUMENT_COUNT_MASK: u64 = 0xFFFF;
const COUNTED_BIT: u64 = 1 << 24;
const RETAINS_REGISTERS_BIT: u64 = 1 << 25;
#[cfg(debug_assertions)]
const GENERATION_SHIFT: u32 = 26;
#[cfg(debug_assertions)]
const GENERATION_MASK: u64 = 0x7FFF_FFFF;
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
    /// The arguments copied into the first registers: at most the
    /// register ceiling, `u16::MAX`, since every argument is a register.
    pub(super) argument_count: u16,
    /// Whether the activation counts against the depth limit.
    pub(super) counted: bool,
    /// Whether a frame object married to the activation keeps its
    /// registers when the activation returns.
    pub(super) retains_registers: bool,
    /// The activation's generation, which a debug build checks every
    /// `ActivationReference` against; 31 bits.
    #[cfg(debug_assertions)]
    pub(super) generation: u32,
}

impl FrameFlags {
    /// The flags of an activation of kind `kind_tag` handed
    /// `argument_count` arguments; a debug build's generation is 0 until
    /// `with_generation` names it.
    #[inline(always)]
    pub(super) fn new(
        kind_tag: u8,
        argument_count: u16,
        counted: bool,
        retains_registers: bool,
    ) -> FrameFlags {
        FrameFlags {
            kind_tag,
            argument_count,
            counted,
            retains_registers,
            #[cfg(debug_assertions)]
            generation: 0,
        }
    }

    /// These flags with the generation `generation`, which must fit 31
    /// bits.
    #[cfg(debug_assertions)]
    #[inline(always)]
    pub(super) fn with_generation(self, generation: u32) -> FrameFlags {
        debug_assert!(
            u64::from(generation) <= GENERATION_MASK,
            "internal: a generation fits 31 bits"
        );
        FrameFlags { generation, ..self }
    }

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
        #[cfg(debug_assertions)]
        {
            payload |= (u64::from(self.generation) & GENERATION_MASK) << GENERATION_SHIFT;
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
            argument_count: ((payload >> ARGUMENT_COUNT_SHIFT) & ARGUMENT_COUNT_MASK) as u16,
            counted: payload & COUNTED_BIT != 0,
            retains_registers: payload & RETAINS_REGISTERS_BIT != 0,
            #[cfg(debug_assertions)]
            generation: ((payload >> GENERATION_SHIFT) & GENERATION_MASK) as u32,
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
/// while it lives. Everything it lends is `[0, top)`, and the caller of a
/// push writes every word of the frame it hands out but the arguments a
/// callee shares with its sender. The words above the top come below it
/// again only when a pop gives a sender back its window above a callee
/// that lay on its slots (`raise_top_over_dead_registers`), which writes
/// every one of them.
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
        debug_assert!(
            limit <= MAXIMUM_STACK_WORDS,
            "internal: a stack's reservation keeps every frame offset inside 32 bits"
        );
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

    /// The end of the committed range, in words: no frame's window reaches
    /// past it.
    pub(super) fn committed(&self) -> usize {
        self.committed
    }

    /// The most words the stack can ever hold.
    #[cfg(test)]
    pub(super) fn limit(&self) -> usize {
        self.limit
    }

    /// Pushes a frame of `words` words and answers its word offset, the
    /// words of every frame below it, which a send's arguments are copied
    /// from, and its own words; `ReservationExhausted`, the top untouched,
    /// when the reservation cannot hold it. The frame's words are whatever
    /// the stack last held there -- a popped frame's, or zero where nothing
    /// was yet -- so the caller writes every one of them before anything
    /// reads the stack again: a word a popped frame left could name an
    /// object since moved or reclaimed. One comparison against the
    /// committed end and the slices' own, with nothing written twice;
    /// committing more is the cold path's.
    #[inline(always)]
    pub(super) fn push(
        &mut self,
        words: usize,
    ) -> Result<(usize, &[u64], &mut [u64]), ReservationExhausted> {
        let start = self.top;
        let end = start + words;
        if end > self.ordinary_end {
            self.grow(words, self.ordinary_limit)?;
        }
        self.top = end;
        let (below, frame) = self.reservation.accessible_mut()[..end].split_at_mut(start);
        Ok((start, below, frame))
    }

    /// Pushes `words` words from `start`, which must not lie above the top
    /// -- a callee's header laid on its sender's argument slots, over the
    /// sender's registers above them, which are dead -- and answers them;
    /// `ReservationExhausted`, the top untouched, when the reservation's
    /// ordinary room cannot hold them. The top becomes their end, below
    /// the sender's window end when the callee's window is the shorter: the
    /// words between are dead, and untraced, until the callee's pop writes
    /// them (`raise_top_over_dead_registers`). One comparison against the
    /// committed end, as `push` makes.
    #[inline(always)]
    pub(super) fn push_from(
        &mut self,
        start: usize,
        words: usize,
    ) -> Result<&mut [u64], ReservationExhausted> {
        debug_assert!(
            start <= self.top,
            "internal: a frame laid on its sender's slots starts below the top"
        );
        let end = start + words;
        if end > self.ordinary_end {
            self.grow_to(end, self.ordinary_limit)?;
        }
        self.top = end;
        Ok(&mut self.reservation.accessible_mut()[start..end])
    }

    /// Lowers the top to `top`, which must not lie above it: a pop of a
    /// frame pushed above its sender's window. The words above the new top
    /// keep whatever they held, but nothing reads them: `words` lends only
    /// below the top, and a push overwrites them.
    #[inline(always)]
    pub(super) fn lower_top(&mut self, top: usize) {
        debug_assert!(top <= self.top, "internal: a pop never raises the top here");
        self.top = top;
    }

    /// Sets the top to `top` and every word from `dead` up to it to
    /// `filler`: a pop of a callee that lay on its sender's slots from
    /// `dead`, its header's offset, gives the sender back its window up to
    /// `top`, its registers above the slots dead. Each is written, since a
    /// word the callee or a frame above it left there would otherwise be
    /// traced again -- keeping alive what nothing reaches, or naming,
    /// after a collection that ran while it lay above the top, an object
    /// since moved or reclaimed.
    #[inline(always)]
    pub(super) fn raise_top_over_dead_registers(&mut self, dead: usize, top: usize, filler: u64) {
        debug_assert!(
            dead <= top && top <= self.committed,
            "internal: a sender's dead registers lie inside the committed stack"
        );
        self.top = top;
        self.reservation.accessible_mut()[dead..top].fill(filler);
    }

    /// `push`, but a frame may end anywhere in the reservation, the reserve
    /// included: the push of an activation the depth limit's reserve is
    /// held for. Cold, since only the unwinding of an overflow makes it.
    #[cold]
    #[inline(never)]
    pub(super) fn push_reserved(
        &mut self,
        words: usize,
    ) -> Result<(usize, &[u64], &mut [u64]), ReservationExhausted> {
        let start = self.top;
        let end = start + words;
        if end > self.committed {
            self.grow(words, self.limit)?;
        }
        self.top = end;
        let (below, frame) = self.reservation.accessible_mut()[..end].split_at_mut(start);
        Ok((start, below, frame))
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
    /// Refuses, committing nothing, past `ceiling` words. A commit the
    /// operating system refuses outright is running out of memory, as the
    /// object space's is.
    #[cold]
    #[inline(never)]
    fn grow(&mut self, words: usize, ceiling: usize) -> Result<(), ReservationExhausted> {
        match self.top.checked_add(words) {
            Some(end) => self.grow_to(end, ceiling),
            None => Err(ReservationExhausted),
        }
    }

    /// `grow`, for a frame that ends at `needed`.
    #[cold]
    #[inline(never)]
    fn grow_to(&mut self, needed: usize, ceiling: usize) -> Result<(), ReservationExhausted> {
        if needed > ceiling {
            return Err(ReservationExhausted);
        }
        let page = Reservation::page_size();
        let size = self.reservation.size();
        let committed_bytes = self.committed * 8;
        // The reservation's size is a whole number of pages, so rounding an
        // end inside it up to a page stays inside it.
        let needed_bytes = (needed * 8).div_ceil(page) * page;
        let growth = committed_bytes.clamp(MINIMUM_GROWTH, MAXIMUM_GROWTH);
        let generous = (committed_bytes + growth)
            .min(ceiling * 8)
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
    /// so it makes one bounds check, against the committed end (the
    /// reservation's accessible words); that
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

    /// The header of the live frame at `offset`: one bounds check, against
    /// the reservation's accessible words -- its committed read-write
    /// prefix, which is `committed` -- for all of its words, as `word`
    /// makes one for a single word. That the frame is live is the caller's, which a debug
    /// build checks here.
    #[inline(always)]
    pub(super) fn header(&self, offset: usize) -> &[u64; frame_header::COUNT] {
        debug_assert!(
            offset + frame_header::COUNT <= self.top,
            "internal: a frame's header lies below the top"
        );
        match self
            .reservation
            .accessible()
            .get(offset..offset + frame_header::COUNT)
            .and_then(|header| header.try_into().ok())
        {
            Some(header) => header,
            None => unreachable!("internal: a live frame's header lies inside the committed stack"),
        }
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

    /// The generations a debug build's flags word must carry: the least
    /// and the most the 31-bit counter reaches.
    #[cfg(debug_assertions)]
    const GENERATIONS: [u32; 2] = [0, 0x7FFF_FFFF];
    #[cfg(not(debug_assertions))]
    const GENERATIONS: [u32; 1] = [0];

    #[test]
    fn flags_round_trip_through_a_small_integer_word() {
        for kind_tag in ALL_KIND_TAGS {
            for argument_count in [0, 1, u16::MAX] {
                for counted in [false, true] {
                    for retains_registers in [false, true] {
                        for generation in GENERATIONS {
                            let flags = FrameFlags::new(
                                kind_tag,
                                argument_count,
                                counted,
                                retains_registers,
                            );
                            #[cfg(debug_assertions)]
                            let flags = flags.with_generation(generation);
                            let _ = generation;
                            let word = flags.word();
                            assert!(word.is_non_negative_small_integer());
                            assert_eq!(FrameFlags::of(word), flags);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_flags_word_packs_its_fields_where_the_layout_says() {
        let payload = |flags: FrameFlags| number_of(flags.word());
        assert_eq!(
            payload(FrameFlags::new(kind_tag::FACTORY, 0, false, false)),
            7
        );
        assert_eq!(
            payload(FrameFlags::new(kind_tag::METHOD, u16::MAX, false, false)),
            0xFFFF << 8
        );
        assert_eq!(
            payload(FrameFlags::new(kind_tag::METHOD, 0, true, false)),
            1 << 24
        );
        assert_eq!(
            payload(FrameFlags::new(kind_tag::METHOD, 0, false, true)),
            1 << 25
        );
        #[cfg(debug_assertions)]
        assert_eq!(
            payload(
                FrameFlags::new(kind_tag::METHOD, 0, false, false).with_generation(0x7FFF_FFFF)
            ),
            0x7FFF_FFFF << 26
        );
    }

    #[test]
    fn the_header_is_thirteen_words_in_every_build() {
        assert_eq!(frame_header::COUNT, 13);
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
        let (offset, _, _) = stack.push(frame_words).expect("push");
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
                Ok((offset, _, frame)) => {
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
                Ok((offset, _, _)) => {
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
        stack.lower_top(ordinary_top);
        assert!(stack.push(frame_words).is_err());
        stack.lower_top(ordinary_top - frame_words);
        assert!(stack.push(frame_words).is_ok());
    }

    #[test]
    fn lowering_the_top_then_pushing_reuses_the_offset() {
        let mut stack = FrameStack::new(1 << 20).expect("reserve");
        let frame_words = frame_header::COUNT + 4;
        stack.push(frame_words).expect("push");
        let (offset, _, _) = stack.push(frame_words).expect("push");
        for word in &mut stack.words_mut()[offset..] {
            *word = number_word(7).bits();
        }
        stack.lower_top(offset);
        assert_eq!(stack.top(), offset);
        assert_eq!(stack.words().len(), offset);
        let (again, _, _) = stack.push(frame_words).expect("push");
        assert_eq!(again, offset);
        assert_eq!(stack.words().len(), offset + frame_words);
    }
}
