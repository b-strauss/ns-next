//! The code zone: one reservation the JIT's machine code lives in, mapped
//! twice -- an executable view code runs from and a writable view the one
//! writer stores through -- so that no page is writable and executable at
//! once (docs/design/execution.md, "Executable memory"). On Apple AArch64
//! the two views are one `MAP_JIT` mapping, and each write switches its
//! thread to writing and back.
//!
//! A zone is reserved with its one writer, which owns it, and shared with
//! whatever runs it as an `Rc<CodeZone>` (`CodeWriter::zone`): the writer
//! stores only through the writable view, and nothing borrows the
//! executable view's bytes, so code is installed while machine code
//! entered from the zone runs -- from a leaf it calls.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::platform;
use crate::{MemoryError, Operation};

/// A byte offset into a code zone. Machine code holds offsets rather than
/// addresses wherever it can, so a body names the same code in whichever
/// view it is read through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CodeOffset(pub u32);

/// A Rust function machine code calls through the shim: the context the
/// code was entered with, and three argument words; it answers one word.
pub type Leaf<C> = fn(&mut C, [u64; 3]) -> u64;

/// The words of the block machine code is entered with. Word 0 is the
/// context's address, word 1 the shim's, word 2 the leaf table's, word 3
/// the number of leaves; `CodeZone::enter` writes these four on every
/// entry. Words 4 to 31 belong to the caller (`jit::stubs` defines them).
pub const ENTRY_BLOCK_WORDS: usize = 32;

/// The block machine code is entered with (`ENTRY_BLOCK_WORDS`), kept from
/// one entry to the next, so that its caller writes a word only when what
/// the word names changes rather than at every entry. Its clones share the
/// one allocation.
///
/// The words are `Cell`s behind an `Rc` that is never lent mutably: Rust
/// reads and writes them only through shared references to
/// interior-mutable cells, which assert no uniqueness, and machine code
/// through the pointer `CodeZone::enter` hands it, derived from the `Rc`'s
/// own pointer without a reference in between, so neither side's access
/// invalidates the other's. An `Rc` rather than a `Box`: a `Box` is
/// retagged as unique wherever it moves, as a `&mut` is, which would
/// invalidate the pointer a later entry hands the code; an `Rc`'s pointer
/// never is. And the block is an allocation of its own, never words
/// inside the context `enter` lends the code, which may hold a clone of
/// it: a leaf's `&mut` context covers the `Rc`, not the words it points
/// to.
#[derive(Clone)]
pub struct EntryBlock {
    words: Rc<[Cell<u64>; ENTRY_BLOCK_WORDS]>,
}

impl EntryBlock {
    /// A block whose every word is 0.
    pub fn new() -> EntryBlock {
        EntryBlock {
            words: Rc::new(std::array::from_fn(|_| Cell::new(0))),
        }
    }

    /// Word `index`. Panics on an index at or past `ENTRY_BLOCK_WORDS`.
    #[inline(always)]
    pub fn get(&self, index: usize) -> u64 {
        self.words[index].get()
    }

    /// Writes `word` at `index`. Panics on an index at or past
    /// `ENTRY_BLOCK_WORDS`.
    #[inline(always)]
    pub fn set(&self, index: usize, word: u64) {
        self.words[index].set(word);
    }

    /// The pointer machine code reads and writes the words through: the
    /// `Rc`'s own, whose provenance covers every word, cast to the words'
    /// representation -- a `Cell<u64>` is laid out as a `u64`.
    fn as_mut_ptr(&self) -> *mut u64 {
        Rc::as_ptr(&self.words).cast::<u64>().cast_mut()
    }
}

impl Default for EntryBlock {
    fn default() -> EntryBlock {
        EntryBlock::new()
    }
}

/// The next zone's identity. It starts at 1, so 0 is never an identity and
/// can stand for "no zone" where one is recorded.
static NEXT_IDENTITY: AtomicU64 = AtomicU64::new(1);

/// One code zone: its two views and its size. What of it is committed is
/// its writer's to know.
pub struct CodeZone {
    mapping: platform::CodeMapping,
    size: usize,
    identity: u64,
}

impl CodeZone {
    /// The largest zone: AArch64's `bl` reaches 128 MiB either way, so every
    /// call from one body of the zone to another, or to a stub at its base,
    /// is a direct call on both architectures.
    pub const MAXIMUM_BYTES: usize = 128 << 20;

    /// Reserves a zone of at least `bytes`, rounded up to the platform's
    /// reservation granularity, committing none of it, and answers its one
    /// writer, which owns it. `InvalidRange` for 0 or more than
    /// `MAXIMUM_BYTES`, so that every offset into a zone fits a `u32`.
    pub fn reserve(bytes: usize) -> Result<CodeWriter, MemoryError> {
        if bytes == 0 || bytes > Self::MAXIMUM_BYTES {
            return Err(MemoryError::InvalidRange);
        }
        let granularity = platform::reservation_granularity();
        let size = bytes.div_ceil(granularity) * granularity;
        if size > Self::MAXIMUM_BYTES {
            return Err(MemoryError::InvalidRange);
        }
        // SAFETY: mapping touches no memory the program owns, and `size` is
        // a multiple of the reservation granularity.
        let mapping = unsafe { platform::map_code(size) }.map_err(|code| MemoryError::Refused {
            operation: Operation::Map,
            code,
        })?;
        Ok(CodeWriter {
            zone: Rc::new(CodeZone {
                mapping,
                size,
                identity: NEXT_IDENTITY.fetch_add(1, Ordering::Relaxed),
            }),
            committed: 0,
        })
    }

    /// The reserved size in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// This zone's identity, unique among the zones of the process and
    /// never 0, so that what records a zone's code can tell whether it
    /// still names this zone.
    pub fn identity(&self) -> u64 {
        self.identity
    }

    /// The address code at `offset` runs from, its provenance exposed.
    /// Panics on an offset past the zone, in every build: what is called
    /// at the answer would be memory the zone does not own.
    pub fn executable_address(&self, offset: CodeOffset) -> usize {
        assert!(
            (offset.0 as usize) < self.size,
            "an offset past the code zone"
        );
        self.mapping
            .executable
            .as_ptr()
            .wrapping_add(offset.0 as usize)
            .expose_provenance()
    }

    /// Runs the code at `stub` with the platform's C calling convention, as
    /// an `extern "C" fn(*mut u64) -> u64` given the block's address, and
    /// answers what it answers. Words 0 to 3 of the block are written first
    /// (`ENTRY_BLOCK_WORDS`), so the code reaches `context` and `leaves`
    /// only through the shim; every other word is as the caller left it,
    /// and as the code leaves it once it returns.
    ///
    /// Safe on one trust: the code at `stub` was written by `ns-runtime`'s
    /// JIT, which is the contract -- a complete function of the C calling
    /// convention that touches only the block, its own frame and what its
    /// leaves hand it, and returns, and that a leaf installing code while
    /// it runs never writes over code it will still run -- installs append.
    /// That is `unsafe` behind a safe abstraction, as
    /// `docs/design/README.md`, "Principles", accepts: no caller can check
    /// machine code, so no caller is asked to. Panics, as
    /// `executable_address` does, on a stub past the zone. A write from a
    /// leaf meanwhile breaks no borrow: it stores through the writable
    /// view, a pointer of the mapping's, and no slice of the executable
    /// view is alive (only `CodeWriter::bytes` lends one, and it borrows
    /// the writer).
    ///
    /// Aliasing: `context` is a `&mut C` held for the whole run, a
    /// protected reference, and the code and the shim reach the context
    /// only through the one pointer derived from it here, so every
    /// `&mut C` the shim hands a leaf is derived from it in turn. Hence
    /// no word the code reads or writes by an address of its own --
    /// between leaf calls, without the shim -- may lie inside `*context`:
    /// that address, taken before the entry, would make the code's access
    /// a foreign one to the protected reference, undefined behaviour once
    /// a leaf wrote the word, though no processor notices and Miri cannot
    /// run machine code to say so. Such words live in allocations the
    /// context only points to, which no retag of it covers and which Rust
    /// never borrows mutably while code runs: `ns-runtime`'s machine words
    /// (`heap/machine_words.rs`, cells behind an `Rc`), and the object
    /// space and the activation stack, reservations of this crate's that
    /// no leaf holds a mutable borrow of once it returns. Their addresses
    /// come from `Reservation::base_address`, the reservation's own
    /// pointer rather than a slice of it, so the slices Rust reborrows
    /// between leaf calls, all derived from that pointer, never invalidate
    /// the address the code holds (that method has the argument in full).
    /// The block is such an allocation too (`EntryBlock`): the code writes
    /// its words through the pointer handed it here, Rust only through its
    /// cells, so the context may hold a clone of it and a leaf may read or
    /// write it.
    ///
    /// A block serves one entry at a time: the code keeps the stack
    /// pointer it returns to and its exit in it, so no leaf may enter code
    /// with the block of the entry it was called from (`ns-runtime`'s
    /// leaves enter none).
    pub fn enter<C>(
        &self,
        stub: CodeOffset,
        block: &EntryBlock,
        context: &mut C,
        leaves: &[Leaf<C>],
    ) -> u64 {
        let address = self.executable_address(stub);
        // The one pointer to the context the code and the shim use: derived
        // once, and `context` is not touched again before the code returns,
        // so every `&mut C` the shim makes from it is the only live one.
        let context: *mut C = context;
        block.set(0, context.expose_provenance() as u64);
        block.set(1, (leaf_shim::<C> as *const ()).expose_provenance() as u64);
        block.set(2, leaves.as_ptr().expose_provenance() as u64);
        block.set(3, leaves.len() as u64);
        let block = block.as_mut_ptr();
        // The view's provenance, which `executable_address` exposed.
        let code = std::ptr::with_exposed_provenance::<()>(address);
        // SAFETY: `code` points into the zone's executable view, which
        // lives while `&self` does; that it holds a complete C function of
        // this signature is the JIT's contract stated above.
        let code =
            unsafe { std::mem::transmute::<*const (), extern "C" fn(*mut u64) -> u64>(code) };
        code(block)
    }
}

impl Drop for CodeZone {
    fn drop(&mut self) {
        // SAFETY: the mapping is this zone's own, `size` bytes long, and
        // `&mut self` in `drop` means nothing refers into it any more.
        unsafe { platform::unmap_code(&self.mapping, self.size) };
    }
}

/// The one way machine code calls Rust: `leaves[leaf](context, [first,
/// second, third])`, the context and the leaf table read from the entry
/// block (`ENTRY_BLOCK_WORDS`), whose address the code hands back as the
/// first argument. It has the platform's C calling convention, so code
/// calls it with the arguments in that convention's registers -- on
/// Windows the fifth on the stack above the 32-byte shadow space.
///
/// Safety argument: machine code entered through `CodeZone::enter` is a
/// function holding `context` mutably; it hands the block back only while
/// it waits for the leaf, so the `&mut C` made here is the only live
/// reference to the context, and the table is the `&[Leaf<C>]` `enter`
/// still borrows. A leaf index at or past the count aborts the process: a
/// panic must never unwind into machine code, and calling past the table
/// would run whatever word lies there. For the same reason a panic in a
/// leaf aborts too -- Rust aborts when a panic reaches an `extern "C"`
/// boundary.
extern "C" fn leaf_shim<C>(block: *mut u64, leaf: u64, first: u64, second: u64, third: u64) -> u64 {
    // SAFETY: `block` is the address `enter` handed the code, of an
    // `EntryBlock`'s `ENTRY_BLOCK_WORDS` cells, which `enter` borrows until
    // the code returns, and the code calls the shim only before it returns;
    // the words are cells, which no shared reference Rust holds to them
    // forbids reading through the `Rc`'s pointer.
    let (context, table, count) =
        unsafe { (block.read(), block.add(2).read(), block.add(3).read()) };
    if leaf >= count {
        std::process::abort();
    }
    let table = std::ptr::with_exposed_provenance::<Leaf<C>>(word_as_address(table));
    // SAFETY: the table is `enter`'s `leaves`, still borrowed, `count` long,
    // and `leaf` is below `count`.
    let function = unsafe { table.add(word_as_address(leaf)).read() };
    let context = std::ptr::with_exposed_provenance_mut::<C>(word_as_address(context));
    // SAFETY: the context pointer `enter` derived once from its `&mut C`,
    // which nothing else uses until the code returns (the argument above).
    function(unsafe { &mut *context }, [first, second, third])
}

/// A block word as an address or an index. Every target the code zone
/// supports has 64-bit addresses, so this never aborts; it aborts rather
/// than truncate because the shim must not panic.
fn word_as_address(word: u64) -> usize {
    usize::try_from(word).unwrap_or_else(|_| std::process::abort())
}

/// The one way code is written into a zone, and its owner: `reserve`
/// makes exactly one per zone. Each write makes what it wrote visible to
/// the instruction fetch, and on Apple AArch64 switches its thread to
/// writing for the write alone, so a writer may live for the zone's whole
/// life beside the code that runs from it. It is neither `Send` nor
/// `Sync`, as the zone it holds by `Rc` is not.
pub struct CodeWriter {
    zone: Rc<CodeZone>,
    /// The bytes from the base the writer committed: code is installed
    /// from the base up, so the committed pages are a prefix, which
    /// `decommit_above` shortens.
    committed: usize,
}

impl CodeWriter {
    /// The zone this writer writes, to share with whatever runs its code.
    pub fn zone(&self) -> &Rc<CodeZone> {
        &self.zone
    }

    /// Stores `bytes` at `offset`, committing the pages it needs, and
    /// flushes the instruction cache over them (nothing on x64).
    /// `InvalidRange`, nothing written, when the range ends past the zone.
    pub fn write(&mut self, offset: CodeOffset, bytes: &[u8]) -> Result<(), MemoryError> {
        let start = offset.0 as usize;
        let end = start
            .checked_add(bytes.len())
            .filter(|end| *end <= self.zone.size)
            .ok_or(MemoryError::InvalidRange)?;
        if bytes.is_empty() {
            return Ok(());
        }
        self.commit_through(end)?;
        platform::begin_code_write();
        // SAFETY: `start..end` is inside the committed prefix, which the
        // writable view maps read-write -- on Apple AArch64 for this thread,
        // switched to writing just above until just below; `bytes` is the
        // caller's own memory and cannot overlap the zone, and no slice of
        // the zone is alive: only `bytes` lends one, borrowing this writer,
        // which `write` holds mutably.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.zone.mapping.writable.as_ptr().wrapping_add(start),
                bytes.len(),
            );
        }
        // SAFETY: the range is inside the zone's committed prefix, flushed
        // at the addresses it runs from.
        unsafe {
            platform::flush_instruction_cache(
                self.zone.mapping.executable.as_ptr().wrapping_add(start),
                bytes.len(),
            );
        }
        platform::end_code_write();
        Ok(())
    }

    /// The bytes of `range` read through the executable view, when the
    /// range is committed; `None` otherwise -- for tests, and for the
    /// disassembly of a body.
    pub fn bytes(&self, range: Range<u32>) -> Option<&[u8]> {
        let start = range.start as usize;
        let end = range.end as usize;
        if start > end || end > self.committed {
            return None;
        }
        // SAFETY: the range is inside the committed prefix, which the
        // executable view maps readable and which is initialized (zero or
        // written); only this writer stores into the zone, and `write`
        // takes it mutably, so no store happens while the slice lives.
        Some(unsafe {
            std::slice::from_raw_parts(
                self.zone.mapping.executable.as_ptr().wrapping_add(start),
                end - start,
            )
        })
    }

    /// The executable view's committed pages are writable: on Windows as
    /// the system reports them now, elsewhere as the view was mapped (see
    /// each platform's `executable_view_is_writable`). Only committed pages
    /// are asked about, so a test writes before it asks.
    #[cfg(test)]
    pub fn executable_view_is_writable(&self) -> bool {
        platform::executable_view_is_writable(&self.zone.mapping, self.committed)
    }

    /// The bytes from the base the writer counts committed, a whole number
    /// of pages: what it commits again before it writes past them. Pages
    /// `decommit_above` gave back are not counted, though the system may
    /// still charge them (`decommit_above`).
    pub fn committed(&self) -> usize {
        self.committed
    }

    /// Gives back the pages from `end`, rounded up to a page, to the
    /// committed prefix's end, and answers how many bytes of them the
    /// system took back: 0 for an end at or past the prefix's end. The
    /// writer stops counting them as committed first, whatever the system
    /// does with them, so that the next write past `end` commits them
    /// again; what they held is gone, and code is never run from them
    /// before a write stores it there again. What the system takes back
    /// differs by platform (`platform::decommit_code`), and only what it
    /// takes is answered: on Windows a section's pages stay charged
    /// against the commit limit, only their physical memory released, and
    /// a POSIX shared memory object outside Linux keeps them, so both
    /// answer 0, as a refusal of the system does.
    pub fn decommit_above(&mut self, end: usize) -> usize {
        let page = platform::page_size();
        let Some(end) = end.checked_next_multiple_of(page) else {
            return 0;
        };
        if end >= self.committed {
            return 0;
        }
        let from = self.committed;
        self.committed = end;
        // SAFETY: `end..from` is page-aligned and inside the zone's
        // committed prefix, and no slice into the zone is alive: only
        // `bytes` lends one, borrowing this writer, which `decommit_above`
        // holds mutably. Nothing runs from the range: its caller, which
        // decides the zone's layout, holds no code there it still runs.
        match unsafe { platform::decommit_code(&self.zone.mapping, end, from - end) } {
            Ok(true) => from - end,
            Ok(false) | Err(_) => 0,
        }
    }

    /// Commits the pages from the committed prefix's end through `end`.
    fn commit_through(&mut self, end: usize) -> Result<(), MemoryError> {
        if end <= self.committed {
            return Ok(());
        }
        let page = platform::page_size();
        // The size is a multiple of the reservation granularity, itself a
        // multiple of the page, so rounding up stays inside the zone.
        let new_end = end.div_ceil(page) * page;
        // SAFETY: `committed..new_end` is page-aligned and inside the zone,
        // past every byte written so far, and no slice into the zone is
        // alive: only `bytes` lends one, borrowing this writer, which
        // `commit_through` holds mutably.
        let outcome = unsafe {
            platform::commit_code(&self.zone.mapping, self.committed, new_end - self.committed)
        };
        outcome.map_err(|code| MemoryError::Refused {
            operation: Operation::Commit,
            code,
        })?;
        self.committed = new_end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_byte_reads_back_through_the_executable_view() {
        let mut zone = CodeZone::reserve(1 << 20).expect("a zone");
        zone.write(CodeOffset(64), &[0xC3, 0x90]).expect("written");
        assert_eq!(zone.bytes(64..66), Some(&[0xC3, 0x90][..]));
    }

    #[test]
    fn a_write_past_the_zone_is_refused() {
        let mut zone = CodeZone::reserve(1 << 16).expect("a zone");
        let end = u32::try_from(zone.zone().size()).expect("small");
        assert!(zone.write(CodeOffset(end - 1), &[0, 0]).is_err());
    }

    #[test]
    fn a_zone_larger_than_the_branch_reach_is_refused() {
        assert!(CodeZone::reserve(CodeZone::MAXIMUM_BYTES + 1).is_err());
    }

    #[test]
    fn two_zones_have_two_identities() {
        let first = CodeZone::reserve(1 << 16).expect("a zone");
        let second = CodeZone::reserve(1 << 16).expect("a zone");
        assert_ne!(first.zone().identity(), second.zone().identity());
        assert_ne!(first.zone().identity(), 0);
    }

    #[test]
    fn the_executable_view_is_never_writable() {
        // On Windows the system's own report of the committed pages
        // (`VirtualQuery`), so the write comes first; elsewhere what the view
        // was mapped with, POSIX having no portable query.
        let mut zone = CodeZone::reserve(1 << 16).expect("a zone");
        zone.write(CodeOffset(0), &[0xC3]).expect("written");
        assert!(zone.bytes(0..1).is_some(), "a page is committed");
        assert!(!zone.executable_view_is_writable());
    }

    #[test]
    fn two_writers_on_one_thread_both_write() {
        // Each write switches the thread on Apple AArch64, whose switch the
        // two writers share, so the first still writes after the second.
        let mut first = CodeZone::reserve(1 << 16).expect("a zone");
        let mut second = CodeZone::reserve(1 << 16).expect("a zone");
        second.write(CodeOffset(0), &[2]).expect("written");
        first.write(CodeOffset(0), &[1]).expect("written");
        second.write(CodeOffset(1), &[3]).expect("written");
        assert_eq!(first.bytes(0..1), Some(&[1][..]));
        assert_eq!(second.bytes(0..2), Some(&[2, 3][..]));
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn code_written_while_the_zone_is_shared_runs() {
        // What a leaf does while code of the zone runs: the zone is shared
        // with the code, and the writer still writes and the code runs.
        let mut writer = CodeZone::reserve(1 << 16).expect("a zone");
        let shared = std::rc::Rc::clone(writer.zone());
        // mov eax, 7; ret
        writer
            .write(CodeOffset(0), &[0xB8, 7, 0, 0, 0, 0xC3])
            .expect("written");
        let address =
            std::ptr::with_exposed_provenance::<()>(shared.executable_address(CodeOffset(0)));
        // SAFETY: the address is the start of the six bytes above, a
        // complete function of the C calling convention, in the zone's
        // executable view, which `shared` keeps alive.
        let function = unsafe { std::mem::transmute::<*const (), extern "C" fn() -> u32>(address) };
        assert_eq!(function(), 7);
    }

    #[test]
    #[should_panic(expected = "an offset past the code zone")]
    fn an_executable_address_past_the_zone_panics() {
        let zone = CodeZone::reserve(1 << 16).expect("a zone");
        let end = u32::try_from(zone.zone().size()).expect("small");
        zone.zone().executable_address(CodeOffset(end));
    }

    #[test]
    fn a_write_across_a_page_boundary_commits_both_pages() {
        let page = platform::page_size();
        let mut zone = CodeZone::reserve(4 * page).expect("a zone");
        let offset = u32::try_from(page - 1).expect("small");
        zone.write(CodeOffset(offset), &[1, 2]).expect("written");
        assert_eq!(zone.bytes(offset..offset + 2), Some(&[1, 2][..]));
        assert_eq!(zone.bytes(0..1), Some(&[0][..]), "committed reads zero");
        let past = u32::try_from(2 * page).expect("small");
        assert!(
            zone.bytes(0..past + 1).is_none(),
            "the third page is not committed"
        );
    }

    /// A decommit gives back the pages above its end and no others, and a
    /// write past it commits them again: what is written there reads back.
    #[test]
    fn a_decommitted_range_is_committed_again_by_a_write() {
        let page = platform::page_size();
        let mut zone = CodeZone::reserve(8 * page).expect("a zone");
        let last = u32::try_from(4 * page - 1).expect("small");
        zone.write(CodeOffset(0), &[1]).expect("written");
        zone.write(CodeOffset(last), &[2]).expect("written");
        assert_eq!(zone.committed(), 4 * page);
        let given_back = zone.decommit_above(page + 1);
        assert!(
            given_back == 2 * page || given_back == 0,
            "from the next page on, or nothing the system takes: {given_back}"
        );
        assert_eq!(zone.committed(), 2 * page);
        assert_eq!(zone.bytes(0..1), Some(&[1][..]), "below the end is kept");
        assert!(zone.bytes(0..last + 1).is_none(), "no longer committed");
        assert_eq!(zone.decommit_above(4 * page), 0, "nothing above the prefix");
        zone.write(CodeOffset(last), &[3]).expect("written again");
        assert_eq!(zone.bytes(last..last + 1), Some(&[3][..]));
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn written_code_runs_from_the_executable_view() {
        let mut zone = CodeZone::reserve(1 << 16).expect("a zone");
        // mov eax, 42; ret
        zone.write(CodeOffset(16), &[0xB8, 42, 0, 0, 0, 0xC3])
            .expect("written");
        let address =
            std::ptr::with_exposed_provenance::<()>(zone.zone().executable_address(CodeOffset(16)));
        // SAFETY: the address is the start of the six bytes above, a
        // complete function of the C calling convention, in the zone's
        // executable view, which lives until the end of the test.
        let function = unsafe { std::mem::transmute::<*const (), extern "C" fn() -> u32>(address) };
        assert_eq!(function(), 42);
    }

    /// `mov rax, [first argument + displacement]; ret`: a body answering one
    /// word of the block it is entered with.
    #[cfg(target_arch = "x86_64")]
    fn a_body_answering_the_block_word(word: u8) -> [u8; 5] {
        #[cfg(windows)]
        let register_byte = 0x41;
        #[cfg(not(windows))]
        let register_byte = 0x47;
        [0x48, 0x8B, register_byte, word * 8, 0xC3]
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn entered_code_answers_a_word_of_its_block() {
        let mut zone = CodeZone::reserve(1 << 16).expect("a zone");
        // mov rax, [argument + 32]; ret
        #[cfg(windows)]
        let code = [0x48, 0x8B, 0x41, 0x20, 0xC3];
        #[cfg(not(windows))]
        let code = [0x48, 0x8B, 0x47, 0x20, 0xC3];
        zone.write(CodeOffset(0), &code).expect("written");
        let block = EntryBlock::new();
        block.set(4, 42);
        let mut context = ();
        assert_eq!(
            zone.zone().enter(CodeOffset(0), &block, &mut context, &[]),
            42
        );
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn the_entry_writes_the_context_and_the_shim_into_the_block() {
        let mut zone = CodeZone::reserve(1 << 16).expect("a zone");
        zone.write(CodeOffset(0), &a_body_answering_the_block_word(0))
            .expect("written");
        zone.write(CodeOffset(16), &a_body_answering_the_block_word(1))
            .expect("written");
        let block = EntryBlock::new();
        let mut context: u64 = 7;
        let context_address = std::ptr::addr_of_mut!(context) as u64;
        assert_eq!(
            zone.zone().enter(CodeOffset(0), &block, &mut context, &[]),
            context_address
        );
        assert_eq!(
            zone.zone().enter(CodeOffset(16), &block, &mut context, &[]),
            (leaf_shim::<u64> as *const ()).expose_provenance() as u64
        );
    }

    #[test]
    fn an_entry_block_is_shared_by_its_clones() {
        let block = EntryBlock::new();
        let clone = block.clone();
        assert_eq!(clone.get(7), 0, "a new block's words are 0");
        block.set(7, 42);
        assert_eq!(clone.get(7), 42);
        clone.set(31, 9);
        assert_eq!(block.get(31), 9);
        assert_eq!(block.as_mut_ptr(), clone.as_mut_ptr());
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn entered_code_writes_the_entry_block_rust_reads() {
        let mut zone = CodeZone::reserve(1 << 16).expect("a zone");
        // mov qword [argument + 40], 42; ret
        #[cfg(windows)]
        let code = [0x48, 0xC7, 0x41, 0x28, 42, 0, 0, 0, 0xC3];
        #[cfg(not(windows))]
        let code = [0x48, 0xC7, 0x47, 0x28, 42, 0, 0, 0, 0xC3];
        zone.write(CodeOffset(0), &code).expect("written");
        let block = EntryBlock::new();
        let clone = block.clone();
        let mut context = ();
        zone.zone().enter(CodeOffset(0), &block, &mut context, &[]);
        assert_eq!(clone.get(5), 42, "the code's write, read through a clone");
    }

    #[test]
    fn the_shim_calls_the_indexed_leaf_with_the_context() {
        // Called from Rust with a block laid out as `enter` lays it out, so
        // the dispatch is checked apart from any call sequence.
        fn first(context: &mut u64, arguments: [u64; 3]) -> u64 {
            *context += arguments[0];
            1
        }
        fn second(context: &mut u64, arguments: [u64; 3]) -> u64 {
            *context += arguments[1] * arguments[2];
            2
        }
        let leaves: [Leaf<u64>; 2] = [first, second];
        let mut context: u64 = 0;
        let block = EntryBlock::new();
        block.set(
            0,
            std::ptr::addr_of_mut!(context).expose_provenance() as u64,
        );
        block.set(2, leaves.as_ptr().expose_provenance() as u64);
        block.set(3, 2);
        let block = block.as_mut_ptr();
        assert_eq!(leaf_shim::<u64>(block, 1, 0, 3, 5), 2);
        assert_eq!(leaf_shim::<u64>(block, 0, 7, 0, 0), 1);
        assert_eq!(context, 22);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn the_entry_writes_the_leaf_table_and_its_count_into_the_block() {
        fn add_one(context: &mut u64, arguments: [u64; 3]) -> u64 {
            *context += 1;
            arguments[0]
        }
        let mut zone = CodeZone::reserve(1 << 16).expect("a zone");
        zone.write(CodeOffset(0), &a_body_answering_the_block_word(2))
            .expect("written");
        zone.write(CodeOffset(16), &a_body_answering_the_block_word(3))
            .expect("written");
        let leaves: [Leaf<u64>; 2] = [add_one, add_one];
        let block = EntryBlock::new();
        let mut context: u64 = 0;
        assert_eq!(
            zone.zone()
                .enter(CodeOffset(0), &block, &mut context, &leaves),
            leaves.as_ptr() as u64
        );
        assert_eq!(
            zone.zone()
                .enter(CodeOffset(16), &block, &mut context, &leaves),
            2
        );
    }
}
