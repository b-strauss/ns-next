//! The baseline JIT: machine code for a body, written against the macro
//! assembler's idioms (`macro_assembler`) and entered and left through the
//! entry and exit stubs (`stubs`) (`docs/design/execution.md`, "Planned:
//! the baseline JIT"). This module holds what lies around the code: the
//! slot on each body that counts its entries and names its machine code
//! (`JitSlot`), the per-interpreter state with the code zone (`JitState`),
//! the compile trigger (`note_entry`), and the run loop's one way into
//! machine code and back (`entered`).
//!
//! Only x64 has an encoder, so everything that emits or runs machine code
//! is compiled for x64 alone; elsewhere `available` answers `false` and
//! every body stays interpreted.
#![cfg_attr(
    not(target_arch = "x86_64"),
    expect(
        dead_code,
        reason = "without an encoder no body is compiled, so nothing installs code or maps it"
    )
)]

#[cfg(target_arch = "x86_64")]
pub(crate) mod baseline;
#[cfg(target_arch = "x86_64")]
pub(crate) mod leaves;
#[cfg(target_arch = "x86_64")]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "only the exit is emitted while every instruction lowers to one; the templates                   and leaves of the next stage call the other idioms"
    )
)]
pub(crate) mod macro_assembler;
#[cfg(target_arch = "x86_64")]
pub(crate) mod stubs;

use std::cell::Cell;
use std::rc::Rc;

use ns_memory::{CodeOffset, CodeZone};

use crate::bytecode::CompiledCode;
use crate::heap::{ActivationReference, RegisterBase};
use crate::interpreter::Mutator;

/// The code zone's size unless a test asks for another. A first guess
/// within the 128 MiB bound: the size is chosen by measurement once
/// bodies run as machine code (`docs/OPEN-WORK.md`, road step 8).
pub(crate) const DEFAULT_ZONE_BYTES: usize = 16 << 20;

/// The entry at which a body is compiled: its second, as Cog compiles a
/// method found in its method cache, so that code run once -- most of a
/// program's initialization -- is never compiled
/// (`docs/design/execution.md`, "When a body is compiled").
const COMPILE_AT_ENTRY: u32 = 2;

/// Every body starts at a multiple of this many bytes of the zone, the
/// processor's instruction fetch block.
const BODY_ALIGNMENT: u32 = 16;

/// Whether this build can compile bodies to machine code.
pub(crate) fn available() -> bool {
    cfg!(target_arch = "x86_64")
}

/// What the JIT has done in one interpreter, counted in every build
/// (`Interpreter::jit_statistics`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JitStatistics {
    /// Bodies compiled to machine code.
    pub compiled_bodies: u64,
    /// Bodies the compiler refused -- too many instructions, or no room
    /// left in the zone -- which stay interpreted for good.
    pub refused_bodies: u64,
    /// Backward jumps machine code took, counted only while
    /// `Interpreter::set_jit_counting` is on, so that a loop in machine
    /// code pays nothing for the count otherwise.
    pub machine_back_edges: u64,
    /// Exits of machine code at a poll that found a safepoint requested.
    pub safepoint_exits: u64,
}

/// What a body's `JitSlot` says once it is compiled.
const COMPILED: u32 = u32::MAX - 1;
/// What a body's `JitSlot` says once it will never be compiled.
const REFUSED: u32 = u32::MAX;

/// A body's state in the JIT: *counting* its entries (`state` below
/// `COMPILED`), *compiled* (`COMPILED`; `zone` names the zone its machine
/// code lies in, `offset` where, `map` its program-counter map), or
/// *refused* (`REFUSED`), after which the body is never looked at again.
/// Interior-mutable, since a body is shared by every activation of it.
///
/// A copy starts counting afresh: it is another body to the JIT, compiled
/// on its own if it is entered often enough.
pub(crate) struct JitSlot {
    state: Cell<u32>,
    /// The identity of the zone the body was compiled into, 0 for none
    /// (`CodeZone::identity`): machine code is entered only from the zone
    /// of the interpreter running the body.
    zone: Cell<u64>,
    offset: Cell<u32>,
    map: Cell<u32>,
}

impl Default for JitSlot {
    fn default() -> JitSlot {
        JitSlot {
            state: Cell::new(0),
            zone: Cell::new(0),
            offset: Cell::new(0),
            map: Cell::new(0),
        }
    }
}

impl Clone for JitSlot {
    fn clone(&self) -> JitSlot {
        JitSlot::default()
    }
}

impl std::fmt::Debug for JitSlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.state.get() {
            REFUSED => formatter.write_str("JitSlot::Refused"),
            COMPILED => formatter
                .debug_struct("JitSlot::Compiled")
                .field("zone", &self.zone.get())
                .field("offset", &self.offset.get())
                .field("map", &self.map.get())
                .finish(),
            entries => write!(formatter, "JitSlot::Counting({entries})"),
        }
    }
}

impl JitSlot {
    /// Whether an entry into the body has anything to do with the JIT:
    /// false only once the body is refused. The one load and compare the
    /// run loop makes on a push (`vm::run_stretch`); everything else is
    /// `entered`'s, out of line.
    #[inline(always)]
    pub(crate) fn is_armed(&self) -> bool {
        self.state.get() != REFUSED
    }

    fn refuse(&self) {
        self.state.set(REFUSED);
        self.zone.set(0);
    }

    fn compiled(&self, zone: u64, offset: CodeOffset, map: u32) {
        self.state.set(COMPILED);
        self.zone.set(zone);
        self.offset.set(offset.0);
        self.map.set(map);
    }
}

/// The machine offset, from the body's start, at which each instruction's
/// code starts, indexed by program counter: where an exit's program
/// counter resumes in machine code, and, backwards, which instruction a
/// return address lies in.
#[derive(Debug)]
pub(crate) struct ProgramCounterMap {
    /// Ascending: instructions are laid out in program-counter order.
    starts: Vec<u32>,
}

impl ProgramCounterMap {
    pub(crate) fn new(starts: Vec<u32>) -> ProgramCounterMap {
        debug_assert!(
            starts.is_sorted(),
            "internal: instructions are laid out in order"
        );
        ProgramCounterMap { starts }
    }

    /// Where the instruction at `program_counter` starts.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "on-stack replacement and the exits that resume in machine code read it"
        )
    )]
    pub(crate) fn machine_offset(&self, program_counter: u32) -> Option<u32> {
        self.starts.get(program_counter as usize).copied()
    }

    /// The instruction whose code starts at `machine_offset`, if one does.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the exit that materializes frames maps return addresses back"
        )
    )]
    pub(crate) fn program_counter_at(&self, machine_offset: u32) -> Option<u32> {
        // Several instructions start at one offset when all but the last
        // emit nothing; the last of them is the one the code there runs.
        let after = self
            .starts
            .partition_point(|start| *start <= machine_offset);
        let index = after.checked_sub(1)?;
        if self.starts[index] != machine_offset {
            return None;
        }
        u32::try_from(index).ok()
    }
}

/// The code zone of one interpreter and what is installed in it.
struct Zone {
    code: Rc<CodeZone>,
    /// The bytes of the zone code may be installed in: its size, or less
    /// when a test asks for a smaller zone than the reservation's
    /// granularity gives.
    limit: u32,
    /// Where the next body goes.
    next_offset: u32,
    entry_stub: CodeOffset,
    exit_stub: CodeOffset,
}

/// Why a body was not compiled. It stays interpreted for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// More instructions than a program counter of the map holds.
    TooManyInstructions,
    /// The assembler could not lay the body out.
    Unassembled,
    /// No room left in the zone, or its memory could not be committed.
    ZoneFull,
}

/// The JIT's state in one interpreter, held by its `Mutator` beside the
/// inline caches, since the run loop reaches only the mutator: the code
/// zone and its stubs, the bodies' program-counter maps, whether the JIT
/// is on, and its statistics.
pub(crate) struct JitState {
    /// `None` when this build has no encoder, the JIT is turned off by
    /// `NS_NEXT_NO_JIT=1`, or the zone could not be reserved.
    zone: Option<Zone>,
    /// The program-counter map of every body compiled into the zone,
    /// indexed by the slot's `map`.
    maps: Vec<ProgramCounterMap>,
    /// Whether bodies are compiled and entered: never without a zone.
    enabled: bool,
    /// Whether machine code counts its backward jumps
    /// (`JitStatistics::machine_back_edges`).
    counting: bool,
    statistics: JitStatistics,
}

impl JitState {
    /// The state of a new interpreter: a zone of `DEFAULT_ZONE_BYTES`,
    /// unless `NS_NEXT_NO_JIT=1` turns the JIT off.
    pub(crate) fn new() -> JitState {
        let wanted =
            available() && std::env::var_os("NS_NEXT_NO_JIT").is_none_or(|value| value != "1");
        let zone = if wanted {
            reserve_zone(DEFAULT_ZONE_BYTES)
        } else {
            None
        };
        JitState {
            enabled: zone.is_some(),
            zone,
            maps: Vec::new(),
            counting: false,
            statistics: JitStatistics::default(),
        }
    }

    pub(crate) fn statistics(&self) -> JitStatistics {
        self.statistics
    }

    /// Turns compiling and entering bodies on or off; on only where a
    /// zone can be reserved. A body entered while the JIT is off is
    /// refused for good, unrecorded in the statistics, so that its later
    /// entries cost the run loop no more than an interpreter without a
    /// JIT pays.
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        if enabled && self.zone.is_none() && available() {
            self.zone = reserve_zone(DEFAULT_ZONE_BYTES);
        }
        self.enabled = enabled && self.zone.is_some();
    }

    /// Replaces the zone with one that installs code in at most `bytes`.
    /// Every body compiled into the old zone is compiled again into the
    /// new one at its next entry, since its slot names the old zone.
    pub(crate) fn replace_zone(&mut self, bytes: usize) {
        self.maps.clear();
        self.zone = None;
        if available() {
            self.zone = reserve_zone(bytes);
        }
        self.enabled = self.enabled && self.zone.is_some();
    }

    pub(crate) fn set_counting(&mut self, counting: bool) {
        self.counting = counting;
    }

    /// The identity of the zone bodies are compiled into, 0 for none.
    fn zone_identity(&self) -> u64 {
        self.zone.as_ref().map_or(0, |zone| zone.code.identity())
    }
}

/// A zone of `bytes` with both stubs installed at its base, or `None`
/// when it cannot be reserved or the stubs do not fit.
#[cfg(target_arch = "x86_64")]
fn reserve_zone(bytes: usize) -> Option<Zone> {
    use macro_assembler::Abi;

    let code = CodeZone::reserve(bytes).ok()?;
    let limit = u32::try_from(bytes.min(code.size())).ok()?;
    let mut zone = Zone {
        code: Rc::new(code),
        limit,
        next_offset: 0,
        entry_stub: CodeOffset(0),
        exit_stub: CodeOffset(0),
    };
    zone.entry_stub = install(&mut zone, &stubs::assemble_entry_stub(Abi::host()).bytes).ok()?;
    zone.exit_stub = install(&mut zone, &stubs::assemble_exit_stub(Abi::host()).bytes).ok()?;
    Some(zone)
}

#[cfg(not(target_arch = "x86_64"))]
fn reserve_zone(_bytes: usize) -> Option<Zone> {
    None
}

/// Writes `bytes` at the zone's next aligned offset and answers it.
fn install(zone: &mut Zone, bytes: &[u8]) -> Result<CodeOffset, Refusal> {
    let offset = zone.next_offset;
    let end = u32::try_from(bytes.len())
        .ok()
        .and_then(|length| offset.checked_add(length))
        .filter(|end| *end <= zone.limit)
        .ok_or(Refusal::ZoneFull)?;
    let Some(code) = Rc::get_mut(&mut zone.code) else {
        panic!("internal: machine code is installed while machine code runs");
    };
    code.writer()
        .write(CodeOffset(offset), bytes)
        .map_err(|_| Refusal::ZoneFull)?;
    zone.next_offset = end
        .checked_next_multiple_of(BODY_ALIGNMENT)
        .unwrap_or(zone.limit);
    Ok(CodeOffset(offset))
}

/// Where `code`'s machine code starts, when it is compiled into this
/// interpreter's zone and the JIT is on: a body compiled by another
/// interpreter, or into a zone since replaced, names a zone that is not
/// this one, and its offset means nothing here.
pub(crate) fn machine_entry(state: &JitState, code: &CompiledCode) -> Option<CodeOffset> {
    let slot = &code.jit;
    (state.enabled && slot.state.get() == COMPILED && slot.zone.get() == state.zone_identity())
        .then(|| CodeOffset(slot.offset.get()))
}

/// Counts an entry into `code`; at its `COMPILE_AT_ENTRY`th, compiles it.
/// A body that cannot be compiled is refused for good. A body compiled
/// into a zone that is not this interpreter's -- another interpreter's, or
/// one since replaced -- is compiled again into this one.
pub(crate) fn note_entry(state: &mut JitState, code: &CompiledCode) {
    let slot = &code.jit;
    if !state.enabled {
        slot.refuse();
        return;
    }
    match slot.state.get() {
        REFUSED => {}
        COMPILED if slot.zone.get() == state.zone_identity() => {}
        COMPILED => compile_into_slot(state, code),
        entries => {
            let entries = entries + 1;
            if entries >= COMPILE_AT_ENTRY {
                compile_into_slot(state, code);
            } else {
                slot.state.set(entries);
            }
        }
    }
}

/// Compiles `code` into the zone and records the outcome in its slot and
/// the statistics.
fn compile_into_slot(state: &mut JitState, code: &CompiledCode) {
    match compile(state, code) {
        Ok((offset, map)) => {
            code.jit.compiled(state.zone_identity(), offset, map);
            state.statistics.compiled_bodies += 1;
        }
        Err(_) => {
            code.jit.refuse();
            state.statistics.refused_bodies += 1;
        }
    }
}

/// Compiles `code` and installs it, answering its offset and the index
/// of its program-counter map.
#[cfg(target_arch = "x86_64")]
fn compile(state: &mut JitState, code: &CompiledCode) -> Result<(CodeOffset, u32), Refusal> {
    let Some(zone) = state.zone.as_mut() else {
        panic!("internal: the JIT is on without a zone");
    };
    let macro_assembler = macro_assembler::MacroAssembler::new(macro_assembler::Abi::host());
    let body = baseline::compile(code, macro_assembler)?;
    let map = u32::try_from(state.maps.len()).map_err(|_| Refusal::ZoneFull)?;
    let offset = install(zone, &body.code.bytes)?;
    state.maps.push(body.map);
    Ok((offset, map))
}

#[cfg(not(target_arch = "x86_64"))]
fn compile(_state: &mut JitState, _code: &CompiledCode) -> Result<(CodeOffset, u32), Refusal> {
    Err(Refusal::Unassembled)
}

/// Where the run loop goes on after a push of a body with an armed slot.
pub(crate) enum Resumption {
    /// Interpreted, at this program counter: the push's own 0 when no
    /// machine code ran, else where machine code left off.
    Interpreted { program_counter: u32 },
    /// Machine code found a safepoint requested before the instruction at
    /// this program counter; the loop takes it there, as a backward jump
    /// that found one does.
    Safepoint { program_counter: u32 },
}

/// A push of `activation`, whose body `code` has an armed slot, at
/// program counter 0: counts the entry, and runs the body's machine code
/// when it has some in this interpreter's zone. Out of line, so that the
/// run loop pays only `JitSlot::is_armed` for the JIT on a push.
#[cold]
#[inline(never)]
pub(crate) fn entered(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    code: &CompiledCode,
    register_base: RegisterBase,
) -> Resumption {
    note_entry(&mut mutator.jit, code);
    match machine_entry(&mutator.jit, code) {
        Some(entry) => run_machine_code(mutator, activation, register_base, entry),
        None => Resumption::Interpreted { program_counter: 0 },
    }
}

/// Runs the machine code at `entry` on `activation`'s frame, whose
/// registers start at `register_base`, and answers where the interpreter
/// goes on. Every address the code needs is asked of the heap afresh, as
/// nothing keeps one valid across entries.
///
/// # Panics
///
/// On a debug build's read of a dead register, with the interpreter's own
/// refusal of one (`Heap::refuse_dead_register`).
#[cfg(target_arch = "x86_64")]
fn run_machine_code(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    register_base: RegisterBase,
    entry: CodeOffset,
) -> Resumption {
    use ns_memory::ENTRY_BLOCK_WORDS;
    use stubs::{BlockWord, MachineExit};

    let Some(zone) = mutator.jit.zone.as_ref() else {
        panic!("internal: machine code is entered without a zone");
    };
    // The zone is held apart from the mutator, which `enter` lends to the
    // code as its context.
    let code = Rc::clone(&zone.code);
    let (entry_stub, exit_stub) = (zone.entry_stub, zone.exit_stub);
    let mut block = [0u64; ENTRY_BLOCK_WORDS];
    block[BlockWord::ObjectSpaceBase.index()] = mutator.heap.object_space_base_address() as u64;
    block[BlockWord::StackBase.index()] = mutator.heap.activation_stack_base_address() as u64;
    block[BlockWord::RegisterBase.index()] = crate::heap::Heap::register_base_words(register_base);
    block[BlockWord::MachineEntry.index()] = code.executable_address(entry) as u64;
    block[BlockWord::PollWord.index()] = mutator.heap.safepoint_flag_address() as u64;
    block[BlockWord::ExitStub.index()] = code.executable_address(exit_stub) as u64;
    let leaves = leaves::table();
    let kind = code.enter(entry_stub, &mut block, mutator, &leaves);
    match stubs::decode_exit(&block, kind) {
        MachineExit::Reexecute { program_counter } => Resumption::Interpreted { program_counter },
        MachineExit::Safepoint { program_counter } => {
            mutator.jit.statistics.safepoint_exits += 1;
            Resumption::Safepoint { program_counter }
        }
        MachineExit::DeadRegister { register, .. } => {
            mutator.heap.refuse_dead_register(activation, register)
        }
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn run_machine_code(
    _mutator: &mut Mutator<'_>,
    _activation: ActivationReference,
    _register_base: RegisterBase,
    _entry: CodeOffset,
) -> Resumption {
    unreachable!("internal: no body has machine code without an encoder")
}

#[cfg(test)]
mod tests {
    use super::{JitSlot, ProgramCounterMap};

    #[test]
    fn the_jit_is_available_exactly_where_machine_code_is_emitted() {
        assert_eq!(super::available(), cfg!(target_arch = "x86_64"));
    }

    #[test]
    fn a_program_counter_map_answers_both_ways() {
        let map = ProgramCounterMap::new(vec![0, 16, 16, 40]);
        assert_eq!(map.machine_offset(1), Some(16));
        assert_eq!(map.machine_offset(4), None);
        assert_eq!(map.program_counter_at(0), Some(0));
        assert_eq!(
            map.program_counter_at(16),
            Some(2),
            "the one whose code runs there"
        );
        assert_eq!(map.program_counter_at(40), Some(3));
        assert_eq!(map.program_counter_at(20), None, "inside an instruction");
        assert_eq!(map.program_counter_at(41), None);
    }

    #[test]
    fn a_copied_slot_counts_afresh() {
        let slot = JitSlot::default();
        slot.compiled(7, ns_memory::CodeOffset(64), 3);
        assert_eq!(
            format!("{slot:?}"),
            "JitSlot::Compiled { zone: 7, offset: 64, map: 3 }"
        );
        assert_eq!(format!("{:?}", slot.clone()), "JitSlot::Counting(0)");
        slot.refuse();
        assert!(!slot.is_armed());
        assert_eq!(format!("{slot:?}"), "JitSlot::Refused");
    }
}
