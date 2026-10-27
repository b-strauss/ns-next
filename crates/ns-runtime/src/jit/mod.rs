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
    /// Bodies refused -- entered while the JIT was off, or refused by the
    /// compiler: too many instructions, code the assembler could not lay
    /// out, or no room left in the zone -- which stay interpreted for
    /// good.
    pub refused_bodies: u64,
    /// Bodies whose first instruction lowers to an exit, so that their
    /// machine code would leave at once: neither installed nor entered,
    /// and interpreted for good.
    pub bodies_exiting_at_entry: u64,
    /// Backward jumps machine code took, counted only by the code compiled
    /// while `Interpreter::set_jit_counting` is on, and only while it is
    /// on, so that a loop in machine code compiled otherwise pays nothing
    /// for the count.
    pub machine_back_edges: u64,
    /// Exits of machine code at a poll that found a safepoint requested.
    pub safepoint_exits: u64,
    /// Entries into machine code, counted on the cold path of a push that
    /// found compiled code, whatever the code did next.
    pub machine_entries: u64,
}

/// What a body's `JitSlot` says once it is compiled.
const COMPILED: u32 = u32::MAX - 2;
/// What a body's `JitSlot` says once its first instruction is known to
/// lower to an exit, so that its machine code is never entered.
const EXITS_AT_ENTRY: u32 = u32::MAX - 1;
/// What a body's `JitSlot` says once it will never be compiled.
const REFUSED: u32 = u32::MAX;

/// A body's state in the JIT: *counting* its entries (`state` below
/// `COMPILED`), *compiled* (`COMPILED`; `zone` names the zone its machine
/// code lies in, `offset` where, `map` its program-counter map), *exiting
/// at entry* (`EXITS_AT_ENTRY`: its code would leave at its first
/// instruction, so entering it would be pure cost on every push), or
/// *refused* (`REFUSED`). A body in either of the last two is never looked
/// at again, so the two are the top values, which one compare tells from
/// the others (`is_armed`).
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
            EXITS_AT_ENTRY => formatter.write_str("JitSlot::ExitsAtEntry"),
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
    /// false once the body is refused or exits at entry. The one load and
    /// compare the run loop makes on a push (`vm::run_stretch`);
    /// everything else is `entered`'s, out of line.
    #[inline(always)]
    pub(crate) fn is_armed(&self) -> bool {
        self.state.get() < EXITS_AT_ENTRY
    }

    fn refuse(&self) {
        self.settle(REFUSED);
    }

    fn settle(&self, state: u32) {
        self.state.set(state);
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
    /// The first instruction lowers to an exit
    /// (`JitStatistics::bodies_exiting_at_entry`).
    ExitsAtEntry,
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
    /// Whether a body exiting at entry is installed all the same, so that
    /// a test fills a zone, or compiles a body anew, and the JIT stress
    /// mode runs the entry stub and the exit on every push, whatever the
    /// body's first instruction.
    install_bodies_exiting_at_entry: bool,
    /// The entry that compiles a body: its first under the JIT stress mode.
    compile_at_entry: u32,
}

/// Whether `NS_NEXT_STRESS_JIT` asks for the JIT stress mode: set, and
/// neither empty nor `0`. Debug builds only, like the other stress modes;
/// read once per process. It forces the JIT on where it is available,
/// compiles every body at its first entry and installs a body whose
/// machine code exits at its first instruction, so that every program runs
/// the entry stub, the exit and the resumption on every push rather than
/// the interpreter alone (`docs/design/README.md`, "Gates").
fn stress_jit_from_env() -> bool {
    static REQUESTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    cfg!(debug_assertions)
        && *REQUESTED.get_or_init(|| {
            std::env::var_os("NS_NEXT_STRESS_JIT")
                .is_some_and(|value| !value.is_empty() && value != "0")
        })
}

/// Whether `NS_NEXT_STRESS_EXIT` asks for the exit stress mode: set, and
/// neither empty nor `0`. Debug builds only; read once per process. Every
/// poll the machine code passes then takes its exit, as if a safepoint
/// were requested: so far every backward jump machine code takes. It
/// forces the JIT on where it is available.
pub(crate) fn stress_exit_from_env() -> bool {
    static REQUESTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    cfg!(debug_assertions)
        && *REQUESTED.get_or_init(|| {
            std::env::var_os("NS_NEXT_STRESS_EXIT")
                .is_some_and(|value| !value.is_empty() && value != "0")
        })
}

impl JitState {
    /// The state of a new interpreter: a zone of `DEFAULT_ZONE_BYTES`,
    /// unless `NS_NEXT_NO_JIT=1` turns the JIT off.
    pub(crate) fn new() -> JitState {
        let stress_jit = stress_jit_from_env();
        let stress = stress_jit || stress_exit_from_env();
        let wanted = available()
            && (stress || std::env::var_os("NS_NEXT_NO_JIT").is_none_or(|value| value != "1"));
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
            install_bodies_exiting_at_entry: stress_jit,
            compile_at_entry: if stress_jit { 1 } else { COMPILE_AT_ENTRY },
        }
    }

    pub(crate) fn statistics(&self) -> JitStatistics {
        self.statistics
    }

    /// Turns compiling and entering bodies on or off; on only where a
    /// zone can be reserved. While it is off, a body is refused at its
    /// first entry, so that every later push of it pays only the run
    /// loop's one compare, as an interpreter without a JIT would; turning
    /// the JIT on again does not revive a body entered meanwhile, so a
    /// caller decides before it runs code.
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

    /// Turns the JIT stress mode on or off for this state, as
    /// `NS_NEXT_STRESS_JIT` does at its creation: every body compiled at
    /// its first entry, and one whose machine code exits at once installed
    /// and entered all the same. Set before the code it is meant for runs.
    pub(crate) fn set_stress(&mut self, stress: bool) {
        self.install_bodies_exiting_at_entry = stress;
        self.compile_at_entry = if stress { 1 } else { COMPILE_AT_ENTRY };
    }

    /// Makes the bodies compiled from now on count their back edges, and
    /// the counts reach the statistics while it is on.
    pub(crate) fn set_counting(&mut self, counting: bool) {
        self.counting = counting;
    }

    /// The identity of the zone bodies are compiled into, 0 for none.
    fn zone_identity(&self) -> u64 {
        self.zone.as_ref().map_or(0, |zone| zone.code.identity())
    }

    /// A state with the JIT on and a zone of `bytes`, whatever the
    /// environment says, installing bodies that exit at entry when
    /// `install_bodies_exiting_at_entry` is set.
    #[cfg(all(test, target_arch = "x86_64"))]
    fn for_test(bytes: usize, install_bodies_exiting_at_entry: bool) -> JitState {
        let zone = reserve_zone(bytes).expect("a zone");
        JitState {
            zone: Some(zone),
            maps: Vec::new(),
            enabled: true,
            counting: false,
            statistics: JitStatistics::default(),
            install_bodies_exiting_at_entry,
            compile_at_entry: COMPILE_AT_ENTRY,
        }
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
/// While the JIT is off, refuses it at once (`JitState::set_enabled`).
/// A body that cannot be compiled is refused for good. A body compiled
/// into a zone that is not this interpreter's -- another interpreter's, or
/// one since replaced -- is compiled again into this one.
pub(crate) fn note_entry(state: &mut JitState, code: &CompiledCode) {
    let slot = &code.jit;
    if !state.enabled {
        slot.refuse();
        state.statistics.refused_bodies += 1;
        return;
    }
    match slot.state.get() {
        REFUSED | EXITS_AT_ENTRY => {}
        COMPILED if slot.zone.get() == state.zone_identity() => {}
        COMPILED => compile_into_slot(state, code),
        entries => {
            let entries = entries + 1;
            if entries >= state.compile_at_entry {
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
        Err(Refusal::ExitsAtEntry) => {
            code.jit.settle(EXITS_AT_ENTRY);
            state.statistics.bodies_exiting_at_entry += 1;
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
    let mut macro_assembler = macro_assembler::MacroAssembler::new(macro_assembler::Abi::host());
    macro_assembler.set_count_back_edges(state.counting);
    let body = baseline::compile(code, macro_assembler)?;
    if body.exits_at_entry && !state.install_bodies_exiting_at_entry {
        return Err(Refusal::ExitsAtEntry);
    }
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

/// The run loop's start at a run's base activation, which the host
/// pushed: `entered`, when the body's slot is armed, as for a push inside
/// the run.
pub(crate) fn entered_at_start(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    code: &CompiledCode,
    register_base: RegisterBase,
) -> Resumption {
    if code.jit.is_armed() {
        entered(mutator, activation, code, register_base)
    } else {
        Resumption::Interpreted { program_counter: 0 }
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
    block[BlockWord::Activation.index()] = crate::heap::Heap::activation_word(activation);
    let leaves = leaves::table();
    mutator.jit.statistics.machine_entries += 1;
    let kind = code.enter(entry_stub, &mut block, mutator, &leaves);
    if mutator.jit.counting {
        mutator.jit.statistics.machine_back_edges += block[BlockWord::BackEdges.index()];
    }
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

    /// The machine-code tests, on the one architecture that emits it.
    #[cfg(target_arch = "x86_64")]
    mod machine {
        use crate::bytecode::{CodeKind, CompiledCode, Instruction, Literal};
        use crate::jit::{machine_entry, note_entry, JitState};

        fn body() -> CompiledCode {
            CompiledCode::new(
                CodeKind::Method,
                vec![
                    Instruction::LoadLiteral {
                        destination: 13,
                        literal: Literal::SmallInteger(3),
                    },
                    Instruction::Return { register: 13 },
                ],
                0,
                14,
                0,
                Vec::new(),
                ns_syntax::Span::new(0, 0),
            )
        }

        /// A body whose first instruction, a return, has no template yet.
        fn body_returning_at_once() -> CompiledCode {
            CompiledCode::new(
                CodeKind::Method,
                vec![Instruction::Return { register: 13 }],
                0,
                14,
                0,
                Vec::new(),
                ns_syntax::Span::new(0, 0),
            )
        }

        fn enter_twice(state: &mut JitState, code: &CompiledCode) {
            note_entry(state, code);
            note_entry(state, code);
        }

        #[test]
        fn a_body_whose_first_instruction_exits_is_never_entered() {
            let mut state = JitState::for_test(1 << 16, false);
            let code = body_returning_at_once();
            enter_twice(&mut state, &code);
            assert_eq!(format!("{:?}", code.jit), "JitSlot::ExitsAtEntry");
            assert!(!code.jit.is_armed(), "a push leaves it alone");
            assert_eq!(machine_entry(&state, &code), None);
            let statistics = state.statistics();
            assert_eq!(statistics.bodies_exiting_at_entry, 1);
            assert_eq!(statistics.compiled_bodies, 0);
            assert_eq!(statistics.refused_bodies, 0);
        }

        #[test]
        fn a_body_whose_first_instruction_has_a_template_is_compiled() {
            let mut state = JitState::for_test(1 << 16, false);
            let code = body();
            enter_twice(&mut state, &code);
            assert!(machine_entry(&state, &code).is_some());
            assert_eq!(state.statistics().compiled_bodies, 1);
            assert_eq!(state.statistics().bodies_exiting_at_entry, 0);
        }

        #[test]
        fn a_disabled_jit_refuses_a_body_at_its_first_entry() {
            let mut state = JitState::for_test(1 << 16, true);
            let code = body();
            state.set_enabled(false);
            note_entry(&mut state, &code);
            assert_eq!(format!("{:?}", code.jit), "JitSlot::Refused");
            assert!(!code.jit.is_armed(), "a later push pays only the compare");
            assert_eq!(state.statistics().refused_bodies, 1);
            state.set_enabled(true);
            note_entry(&mut state, &code);
            assert_eq!(machine_entry(&state, &code), None, "not revived");
            assert_eq!(state.statistics().refused_bodies, 1);
            assert_eq!(state.statistics().compiled_bodies, 0);
        }

        #[test]
        fn a_body_compiled_into_another_zone_is_compiled_again() {
            let mut first = JitState::for_test(1 << 16, true);
            let mut second = JitState::for_test(1 << 16, true);
            let code = body();
            enter_twice(&mut first, &code);
            assert!(machine_entry(&first, &code).is_some());
            assert_eq!(
                machine_entry(&second, &code),
                None,
                "an offset into the first zone means nothing in the second"
            );
            note_entry(&mut second, &code);
            assert!(machine_entry(&second, &code).is_some());
            assert_eq!(machine_entry(&first, &code), None);
            assert_eq!(second.statistics().compiled_bodies, 1);
        }

        #[test]
        fn a_full_zone_refuses_bodies() {
            let mut state = JitState::for_test(4096, true);
            let bodies: Vec<CompiledCode> = (0..200).map(|_| body()).collect();
            for code in &bodies {
                enter_twice(&mut state, code);
            }
            let statistics = state.statistics();
            assert!(statistics.compiled_bodies >= 1);
            assert!(statistics.refused_bodies >= 1, "one page fills");
            let last = bodies.last().expect("a body");
            assert_eq!(format!("{:?}", last.jit), "JitSlot::Refused");
            assert_eq!(machine_entry(&state, last), None);
        }
    }
}
