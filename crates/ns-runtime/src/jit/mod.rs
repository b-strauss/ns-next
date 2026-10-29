//! The baseline JIT: machine code for a body, written against the macro
//! assembler's idioms (`macro_assembler`) and entered and left through the
//! entry and exit stubs (`stubs`), its sends finding what their cells do
//! not hold through the polymorphic and megamorphic stubs (`targets`)
//! (`docs/design/execution.md`, "Planned: the baseline JIT"). This module holds what lies around the code: the
//! slot on each body that counts its entries and names its machine code
//! (`JitSlot`), the per-interpreter state with the code zone (`JitState`),
//! the compile trigger (`note_entry`), and the run loop's one way into
//! machine code and back (`entered`).
//!
//! Everything that emits or runs machine code is compiled only where the
//! build script sets `cfg(ns_jit)` -- a target with an encoder, x64 today
//! (`build.rs`); elsewhere `available` answers `false` and every body
//! stays interpreted.
#![cfg_attr(
    not(ns_jit),
    expect(
        dead_code,
        reason = "without an encoder no body is compiled, so nothing installs code or maps it"
    )
)]

#[cfg(ns_jit)]
pub(crate) mod baseline;
#[cfg(ns_jit)]
pub(crate) mod leaves;
#[cfg(ns_jit)]
pub(crate) mod macro_assembler;
#[cfg(ns_jit)]
pub(crate) mod materialize;
#[cfg(ns_jit)]
pub(crate) mod sends;
#[cfg(ns_jit)]
pub(crate) mod stubs;
#[cfg(ns_jit)]
pub(crate) mod targets;

use std::cell::Cell;
use std::rc::{Rc, Weak};

use ns_memory::{CodeOffset, CodeWriter};

use crate::bytecode::CompiledCode;
use crate::heap::{ActivationReference, CodeId, Heap, RegisterBase};
use crate::interpreter::{FeedbackBase, Mutator, DEPTH_RESERVE, MAX_ACTIVATION_DEPTH};
use crate::mixin::MixinMethod;

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

/// The native stack an entry into machine code needs left: a return
/// address of eight bytes for every activation the depth limit and its
/// reserve count, since compiled frames keep theirs on the native stack,
/// and 64 KiB for the deepest leaf call below the innermost
/// (`docs/design/execution.md`, "The native stack carries the return
/// addresses"). A thread with less runs interpreted.
pub(crate) const NATIVE_STACK_NEED: usize = (MAX_ACTIVATION_DEPTH + DEPTH_RESERVE) * 8 + 64 * 1024;

/// Whether the running thread's native stack has `NATIVE_STACK_NEED` left
/// below the caller; `false` where the platform does not say.
fn native_stack_has_room() -> bool {
    ns_memory::native_stack::remaining_bytes()
        .is_some_and(|remaining| remaining >= NATIVE_STACK_NEED)
}

/// Whether this build can compile bodies to machine code.
pub(crate) fn available() -> bool {
    cfg!(ns_jit)
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
    /// Bodies whose machine code would exit before it reaches a return, a
    /// compiled send or a backward jump, so that it would run at most a few straight-line
    /// instructions for the price of an entry and an exit: neither
    /// installed nor entered, and interpreted for good.
    pub bodies_exiting_at_once: u64,
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
    /// Sends machine code made through a cell whose guard matched, counted,
    /// as `machine_back_edges` is, only by the code compiled while
    /// `Interpreter::set_jit_counting` is on: calls of a callee's machine
    /// entry, or of the stub the cell names until it has one -- whether or
    /// not the callee's checks then let the call through.
    pub compiled_calls: u64,
    /// Calls of the miss leaf (`jit::leaves`), which decides a send machine
    /// code's cell did not.
    pub miss_leaf_calls: u64,
    /// Entries into machine code at a frame returned to, which an exit
    /// above it had materialized (`vm::resume_caller`).
    pub resumed_entries: u64,
    /// Frames machine code pushed that an exit materialized
    /// (`jit::materialize`).
    pub materialized_frames: u64,
    /// Exits of machine code that handed a send to the interpreter: a
    /// compiled call refused -- by a callee's checks, a stub, or a leaf
    /// stub's decline -- and an exit at a send instruction, which runs it
    /// again.
    pub send_exits: u64,
    /// Calls of the leaf stub's leaf (`jit::leaves`'s `act`), which runs a
    /// target that is no method with code -- a setter, a leaf-safe
    /// primitive -- as the interpreter's `act` does.
    pub act_leaf_calls: u64,
}

/// What a body's `JitSlot` says once it is compiled.
const COMPILED: u32 = u32::MAX - 2;
/// What a body's `JitSlot` says once its machine code is known to leave
/// before it reaches a return, a compiled send or a backward jump, so that
/// it is never entered.
const EXITS_AT_ONCE: u32 = u32::MAX - 1;
/// What a body's `JitSlot` says once it will never be compiled.
const REFUSED: u32 = u32::MAX;

/// A body's state in the JIT: *counting* its entries (`state` below
/// `COMPILED`), *compiled* (`COMPILED`; `zone` names the zone its machine
/// code lies in, `offset` where, `map` its program-counter map), *exiting
/// at once* (`EXITS_AT_ONCE`: its code would exit before it reaches a
/// return, a compiled send or a backward jump, so entering it would cost
/// every push more than the few instructions it runs), or
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
            EXITS_AT_ONCE => formatter.write_str("JitSlot::ExitsAtOnce"),
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
    /// false once the body is refused or exits at once. The one load and
    /// compare the run loop makes on a push (`vm::run_stretch`);
    /// everything else is `entered`'s, out of line.
    #[inline(always)]
    pub(crate) fn is_armed(&self) -> bool {
        self.state.get() < EXITS_AT_ONCE
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
/// return address lies in; and the offset of every call's return address
/// with the program counter of the send that calls, which the exit's walk
/// over the return addresses on the native stack reads.
#[derive(Debug)]
pub(crate) struct ProgramCounterMap {
    /// Ascending: instructions are laid out in program-counter order.
    starts: Vec<u32>,
    /// Each call's return address as an offset from the body's start, and
    /// the program counter of the send that calls; strictly ascending by
    /// offset, as the calls are laid out.
    returns: Vec<CallReturn>,
}

/// Where a call in a body returns to, and the send it is the call of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CallReturn {
    /// The return address, as an offset from the body's start.
    pub(crate) machine_offset: u32,
    /// The program counter of the send whose call returns there.
    pub(crate) program_counter: u32,
}

impl ProgramCounterMap {
    pub(crate) fn new(starts: Vec<u32>, returns: Vec<CallReturn>) -> ProgramCounterMap {
        debug_assert!(
            starts.is_sorted(),
            "internal: instructions are laid out in order"
        );
        debug_assert!(
            returns
                .windows(2)
                .all(|pair| pair[0].machine_offset < pair[1].machine_offset),
            "internal: calls are laid out in order, each returning past the last"
        );
        ProgramCounterMap { starts, returns }
    }

    /// The program counter of the send whose call returns to
    /// `machine_offset`, an offset from the body's start; `None` when no
    /// call of the body returns there.
    pub(crate) fn program_counter_of_return(&self, machine_offset: u32) -> Option<u32> {
        let index = self
            .returns
            .binary_search_by_key(&machine_offset, |call| call.machine_offset)
            .ok()?;
        Some(self.returns[index].program_counter)
    }

    /// Where the instruction at `program_counter` starts: where the run
    /// loop enters the body's machine code at a push (program counter 0)
    /// and at a frame returned to (the instruction after its send). The
    /// body's machine entry, where a compiled call enters it, is its
    /// first byte instead, the callee's prologue (`sends::prologue`).
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
    /// The zone's one writer, which owns it: an install writes through
    /// it, from the run loop or from a leaf while machine code runs, and
    /// an entry runs the code from the zone it shares (`CodeWriter::zone`).
    writer: CodeWriter,
    /// The bytes of the zone code may be installed in: its size, or less
    /// when a test asks for a smaller zone than the reservation's
    /// granularity gives.
    limit: u32,
    /// Where the next body goes.
    next_offset: u32,
    entry_stub: CodeOffset,
    exit_stub: CodeOffset,
    /// The stub a cell names for a method whose body has no machine code
    /// yet (`stubs::assemble_miss_stub`).
    miss_stub: CodeOffset,
    /// The stub a cell names for a target machine code does not run
    /// (`stubs::assemble_refusing_stub`).
    refusing_stub: CodeOffset,
    /// The stub a send whose guard is not its cell's calls, and which a
    /// polymorphic site's cell names (`targets::assemble_polymorphic_stub`).
    polymorphic_stub: CodeOffset,
    /// The stub a megamorphic site's cell names
    /// (`targets::assemble_megamorphic_stub`).
    megamorphic_stub: CodeOffset,
    /// The stubs a cell names for a target that is no method with code
    /// (`targets::assemble_accessor_stubs`).
    accessors: AccessorStubs,
}

/// Where each accessor stub lies in the zone
/// (`targets::assemble_accessor_stubs`).
struct AccessorStubs {
    /// The getter of each field that has one, by field.
    getters: Vec<CodeOffset>,
    /// The quick method answering each parameter position that has one,
    /// by position.
    parameters: Vec<CodeOffset>,
    nil: CodeOffset,
    true_constant: CodeOffset,
    false_constant: CodeOffset,
    receiver: CodeOffset,
    /// The leaf stub, which runs `act`'s arm through the leaf shim.
    act: CodeOffset,
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
    /// The code leaves before it can take a backward jump
    /// (`JitStatistics::bodies_exiting_at_once`).
    ExitsAtOnce,
}

/// A body installed in the zone: which body, where its code lies, and its
/// program-counter map.
struct InstalledBody {
    /// The body itself, held weakly so that its code still dies with its
    /// mixin's row: the zone keeps no body alive. Not its code id, which a
    /// full collection may free and mint again for another body; the id is
    /// asked of the heap whenever it is wanted (`JitState::body_at`). An
    /// allocation a `Weak` points to is never reused while the `Weak`
    /// lives, so a body that died answers nothing rather than another.
    code: Weak<CompiledCode>,
    /// The method the body is the body of, when it is one's, held weakly
    /// as the body is: a frame materialized for it names it through its
    /// code's row (`materialize`).
    method: Option<Weak<MixinMethod>>,
    /// The feedback base the body's sends' cells were compiled against:
    /// its machine code is entered only by an activation with that base.
    feedback_base: FeedbackBase,
    /// The offsets of its first byte and of the byte past its last.
    start: u32,
    end: u32,
    map: ProgramCounterMap,
}

/// The JIT's state in one interpreter, held by its `Mutator` beside the
/// inline caches, since the run loop reaches only the mutator: the code
/// zone and its stubs, the bodies installed in it, whether the JIT is on,
/// and its statistics.
pub(crate) struct JitState {
    /// `None` when this build has no encoder, the JIT is turned off by
    /// `NS_NEXT_NO_JIT=1`, or the zone could not be reserved.
    zone: Option<Zone>,
    /// Every body compiled into the zone, indexed by the slot's `map`, in
    /// the order they were installed, which is ascending by offset: an
    /// install appends.
    bodies: Vec<InstalledBody>,
    /// Whether bodies are compiled and entered: never without a zone.
    enabled: bool,
    /// Whether machine code counts its backward jumps
    /// (`JitStatistics::machine_back_edges`).
    counting: bool,
    statistics: JitStatistics,
    /// Whether a body exiting at once is installed all the same, so
    /// that a test fills a zone, or compiles a body anew, and the JIT
    /// stress mode runs the entry stub and the exit on every push, whatever
    /// the body's code.
    install_bodies_exiting_at_once: bool,
    /// The entry that compiles a body: its first under the JIT stress mode.
    compile_at_entry: u32,
    /// The exit's walk over the compiled frames' return addresses, while
    /// the exit stub makes it (`materialize`).
    pub(crate) walk: materialize::Walk,
    /// The innermost frame the last walk materialized, for the run loop to
    /// go on with (`take_walked`).
    pub(crate) walked: Option<ActivationReference>,
}

/// Whether `NS_NEXT_STRESS_JIT` asks for the JIT stress mode: set, and
/// neither empty nor `0`. Debug builds only, like the other stress modes;
/// read once per process. It forces the JIT on where it is available,
/// compiles every body at its first entry and installs a body whose
/// machine code exits at once, so that every program runs
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
            bodies: Vec::new(),
            counting: false,
            statistics: JitStatistics::default(),
            install_bodies_exiting_at_once: stress_jit,
            compile_at_entry: if stress_jit { 1 } else { COMPILE_AT_ENTRY },
            walk: materialize::Walk::default(),
            walked: None,
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
    ///
    /// Answers whether a zone was reserved, which no cell filled before
    /// names: the caller vacates the caches, whose cells answered 0 while
    /// there was none.
    pub(crate) fn set_enabled(&mut self, enabled: bool) -> bool {
        let reserved = enabled && self.zone.is_none() && available();
        if reserved {
            self.zone = reserve_zone(DEFAULT_ZONE_BYTES);
        }
        self.enabled = enabled && self.zone.is_some();
        reserved
    }

    /// Replaces the zone with one that installs code in at most `bytes`.
    /// Every body compiled into the old zone is compiled again into the
    /// new one at its next entry, since its slot names the old zone. A
    /// cell that names an address in the old zone must be vacated with it
    /// (`Interpreter::set_jit_zone_bytes` invalidates the caches).
    pub(crate) fn replace_zone(&mut self, bytes: usize) {
        self.bodies.clear();
        self.zone = None;
        if available() {
            self.zone = reserve_zone(bytes);
        }
        self.enabled = self.enabled && self.zone.is_some();
    }

    /// Turns the JIT stress mode on or off for this state, as
    /// `NS_NEXT_STRESS_JIT` does at its creation: every body compiled at
    /// its first entry, and one whose machine code exits at once
    /// installed and entered all the same. Set before the code it is meant
    /// for runs.
    pub(crate) fn set_stress(&mut self, stress: bool) {
        self.install_bodies_exiting_at_once = stress;
        self.compile_at_entry = if stress { 1 } else { COMPILE_AT_ENTRY };
    }

    /// Makes the bodies compiled from now on count their back edges, and
    /// the counts reach the statistics while it is on.
    pub(crate) fn set_counting(&mut self, counting: bool) {
        self.counting = counting;
    }

    /// The identity of the zone bodies are compiled into, 0 for none.
    fn zone_identity(&self) -> u64 {
        self.zone
            .as_ref()
            .map_or(0, |zone| zone.writer.zone().identity())
    }

    /// The body a call returns into at `return_address`, an address in the
    /// zone's executable view that the exit's walk finds on the native
    /// stack, with the body's code id in `heap` and its program-counter
    /// map, whose offsets count from the body's start (`body_start`).
    /// `None` for an address in no installed body -- the stubs' among them
    /// -- and for a body that has died since it was installed. The id is
    /// asked of the heap afresh, which mints one for a body a full
    /// collection freed the id of, so it never names another body; a
    /// return address is looked up by the byte before it, the call's last,
    /// so that a call ending its body still finds its body rather than the
    /// next one installed right after it.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the exit that materializes frames maps return addresses back"
        )
    )]
    pub(crate) fn body_at(
        &self,
        return_address: usize,
        heap: &mut Heap<'_>,
    ) -> Option<(CodeId, &ProgramCounterMap)> {
        let body = self.installed_body_at(return_address)?;
        let code = body.code.upgrade()?;
        Some((heap.code_id(&code), &body.map))
    }

    /// Where the code of the body a call returns into at `return_address`
    /// starts, as an address in the zone's executable view: what a return
    /// address less gives the offset its body's program-counter map is read
    /// at. Looked up as `body_at` looks it up.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the exit that materializes frames maps return addresses back"
        )
    )]
    pub(crate) fn body_start(&self, return_address: usize) -> Option<usize> {
        let body = self.installed_body_at(return_address)?;
        let zone = self.zone.as_ref()?;
        Some(
            zone.writer
                .zone()
                .executable_address(CodeOffset(body.start)),
        )
    }

    /// For each installed body, a return address into it -- the address
    /// one past its start -- and the body, while it lives.
    #[cfg(test)]
    pub(crate) fn installed_bodies_for_test(&self) -> Vec<(usize, Option<Rc<CompiledCode>>)> {
        let Some(zone) = self.zone.as_ref() else {
            return Vec::new();
        };
        self.bodies
            .iter()
            .map(|body| {
                let start = zone
                    .writer
                    .zone()
                    .executable_address(CodeOffset(body.start));
                (start + 1, body.code.upgrade())
            })
            .collect()
    }

    /// The body a call returns into at `return_address`, its method, the
    /// feedback base it was compiled against, and the program counter of
    /// the send whose call returns there: what a compiled frame's return
    /// address on the native stack says of its caller. `None` for an
    /// address no call of an installed body returns to.
    pub(crate) fn return_site(&self, return_address: usize) -> Option<(Installed, u32)> {
        let body = self.installed_body_at(return_address)?;
        let zone = self.zone.as_ref()?;
        let start = zone
            .writer
            .zone()
            .executable_address(CodeOffset(body.start));
        let offset = u32::try_from(return_address - start).ok()?;
        let program_counter = body.map.program_counter_of_return(offset)?;
        Some((upgraded(body)?, program_counter))
    }

    /// The body an address inside the code lies in, by the byte before it
    /// as `body_at` looks it up -- an address `MacroAssembler::exit` or a
    /// leaf call's `Operand::CodeAddress` handed over -- its method and its
    /// feedback base.
    pub(crate) fn body_of(&self, address: usize) -> Option<Installed> {
        upgraded(self.installed_body_at(address)?)
    }

    /// The sending frame's register 0 and the program counter of the send
    /// whose call returns to `return_address` and whose callee's register
    /// 0 lies at `callee_address`: the callee lies on the send's argument
    /// slots.
    pub(crate) fn sender_of(&self, callee_address: u64, return_address: u64) -> Option<(u64, u32)> {
        let (installed, program_counter) =
            self.return_site(usize::try_from(return_address).ok()?)?;
        let instruction = installed.code.instruction(program_counter as usize);
        let (argument_base, _) = instruction.argument_slots()?;
        let sender = callee_address.checked_sub(u64::from(argument_base) * 8)?;
        Some((sender, program_counter))
    }

    /// The innermost frame the last exit's walk materialized, taken.
    pub(crate) fn take_walked(&mut self) -> Option<ActivationReference> {
        self.walked.take()
    }

    /// The installed body holding the byte before `return_address`.
    fn installed_body_at(&self, return_address: usize) -> Option<&InstalledBody> {
        let zone = self.zone.as_ref()?;
        let base = zone.writer.zone().executable_address(CodeOffset(0));
        let offset = u32::try_from(return_address.checked_sub(base)?.checked_sub(1)?).ok()?;
        // Installed in ascending order, so the body holding the offset, if
        // one does, is the last that starts at or before it.
        let after = self.bodies.partition_point(|body| body.start <= offset);
        let body = &self.bodies[after.checked_sub(1)?];
        (offset < body.end).then_some(body)
    }

    /// A state with the JIT on and a zone of `bytes`, whatever the
    /// environment says, installing bodies that leave before a loop when
    /// `install_bodies_exiting_at_once` is set.
    #[cfg(all(test, ns_jit))]
    fn for_test(bytes: usize, install_bodies_exiting_at_once: bool) -> JitState {
        let zone = reserve_zone(bytes).expect("a zone");
        JitState {
            zone: Some(zone),
            bodies: Vec::new(),
            enabled: true,
            counting: false,
            statistics: JitStatistics::default(),
            install_bodies_exiting_at_once,
            compile_at_entry: COMPILE_AT_ENTRY,
            walk: materialize::Walk::default(),
            walked: None,
        }
    }
}

/// An installed body, while it lives: its code, its method when it is
/// one's, and the feedback base it was compiled against.
pub(crate) struct Installed {
    pub(crate) code: Rc<CompiledCode>,
    pub(crate) method: Option<Rc<MixinMethod>>,
    pub(crate) feedback_base: FeedbackBase,
}

/// `body` as `Installed`, while its code and method live.
fn upgraded(body: &InstalledBody) -> Option<Installed> {
    let code = body.code.upgrade()?;
    let method = match &body.method {
        Some(method) => Some(method.upgrade()?),
        None => None,
    };
    Some(Installed {
        code,
        method,
        feedback_base: body.feedback_base,
    })
}

/// A zone of `bytes` with both stubs installed at its base, or `None`
/// when it cannot be reserved or the stubs do not fit.
#[cfg(ns_jit)]
fn reserve_zone(bytes: usize) -> Option<Zone> {
    use macro_assembler::Abi;

    let writer = ns_memory::CodeZone::reserve(bytes).ok()?;
    let limit = u32::try_from(bytes.min(writer.zone().size())).ok()?;
    let mut zone = Zone {
        writer,
        limit,
        next_offset: 0,
        entry_stub: CodeOffset(0),
        exit_stub: CodeOffset(0),
        miss_stub: CodeOffset(0),
        refusing_stub: CodeOffset(0),
        polymorphic_stub: CodeOffset(0),
        megamorphic_stub: CodeOffset(0),
        accessors: AccessorStubs {
            getters: Vec::new(),
            parameters: Vec::new(),
            nil: CodeOffset(0),
            true_constant: CodeOffset(0),
            false_constant: CodeOffset(0),
            receiver: CodeOffset(0),
            act: CodeOffset(0),
        },
    };
    zone.entry_stub = install(&mut zone, &stubs::assemble_entry_stub(Abi::host()).bytes).ok()?;
    zone.exit_stub = install(&mut zone, &stubs::assemble_exit_stub(Abi::host()).bytes).ok()?;
    zone.miss_stub = install(&mut zone, &stubs::assemble_miss_stub(Abi::host()).bytes).ok()?;
    zone.refusing_stub =
        install(&mut zone, &stubs::assemble_refusing_stub(Abi::host()).bytes).ok()?;
    zone.polymorphic_stub = install(
        &mut zone,
        &targets::assemble_polymorphic_stub(Abi::host()).bytes,
    )
    .ok()?;
    zone.megamorphic_stub = install(
        &mut zone,
        &targets::assemble_megamorphic_stub(Abi::host()).bytes,
    )
    .ok()?;
    let (accessors, entries) = targets::assemble_accessor_stubs(Abi::host());
    let base = install(&mut zone, &accessors.bytes).ok()?.0;
    let placed = |offset: u32| CodeOffset(base + offset);
    zone.accessors = AccessorStubs {
        getters: entries
            .getters
            .iter()
            .map(|offset| placed(*offset))
            .collect(),
        parameters: entries
            .parameters
            .iter()
            .map(|offset| placed(*offset))
            .collect(),
        nil: placed(entries.nil),
        true_constant: placed(entries.true_constant),
        false_constant: placed(entries.false_constant),
        receiver: placed(entries.receiver),
        act: placed(entries.act),
    };
    Some(zone)
}

#[cfg(not(ns_jit))]
fn reserve_zone(_bytes: usize) -> Option<Zone> {
    None
}

/// Writes `bytes` at the zone's next aligned offset and answers it. Machine
/// code may be running from the zone meanwhile -- a leaf installs a callee
/// -- which holds no address an install moves: code is only ever appended.
fn install(zone: &mut Zone, bytes: &[u8]) -> Result<CodeOffset, Refusal> {
    let offset = zone.next_offset;
    let end = u32::try_from(bytes.len())
        .ok()
        .and_then(|length| offset.checked_add(length))
        .filter(|end| *end <= zone.limit)
        .ok_or(Refusal::ZoneFull)?;
    zone.writer
        .write(CodeOffset(offset), bytes)
        .map_err(|_| Refusal::ZoneFull)?;
    zone.next_offset = end
        .checked_next_multiple_of(BODY_ALIGNMENT)
        .unwrap_or(zone.limit);
    Ok(CodeOffset(offset))
}

/// The address a hit on an entry for `target` calls, its cell's `entry`
/// word (`inline_cache::cells::Cell`): a function of the target and of
/// this interpreter's JIT state, never of the send or the receiver, so
/// that the cell copies what the lookup decided. A site state that holds
/// no target of its own names the stub that finds one -- a polymorphic
/// site the polymorphic stub, which scans its block, a megamorphic one
/// the megamorphic stub, which probes the global cache's mirror
/// (`targets`) -- and a vacant one answers 0, as every target does where
/// there is no zone. A method machine code can run --
/// one with no fast path, found on any chain but a `doesNotUnderstand:`
/// handler's, whose body takes the method's arguments -- is reached
/// through its body's machine entry once its body has machine code in
/// this zone compiled against the entry's feedback base, and through the
/// miss stub while its body may still be compiled, which compiles it and
/// rewrites the site's cell (`inline_cache::decide_at`).
///
/// A target that is no method with code is reached through the stub
/// `stub_entry` chooses for it.
pub(crate) fn cell_entry(
    state: &JitState,
    target: &crate::interpreter::inline_cache::Target,
) -> u64 {
    use crate::interpreter::inline_cache::Target;
    let Some(zone) = state.zone.as_ref() else {
        return 0;
    };
    let address = |offset: CodeOffset| zone.writer.zone().executable_address(offset) as u64;
    let stubbed = match target {
        Target::Vacant => return 0,
        Target::Polymorphic { .. } => return address(zone.polymorphic_stub),
        Target::Megamorphic { .. } => return address(zone.megamorphic_stub),
        Target::Method {
            method,
            feedback_base,
            chain,
            ..
        } if runs_in_machine_code(method, *chain) => {
            return match state.machine_entry_of(&method.code, *feedback_base) {
                Some(entry) => address(entry),
                None if state.may_compile(&method.code) => address(zone.miss_stub),
                None => address(zone.refusing_stub),
            };
        }
        Target::Method { method, chain, .. } => Stubbed::Method {
            method,
            chain: *chain,
        },
        Target::Slot {
            kind, chain, field, ..
        } => Stubbed::Slot {
            kind: *kind,
            chain: *chain,
            field: usize::try_from(*field).unwrap_or(usize::MAX),
        },
        Target::Primitive {
            primitive, chain, ..
        } => Stubbed::Primitive {
            primitive: *primitive,
            chain: *chain,
        },
        Target::NestedClass { .. } | Target::Factory { .. } => {
            return address(zone.refusing_stub);
        }
    };
    stub_entry(state, &stubbed).unwrap_or_else(|| address(zone.refusing_stub))
}

/// A target that is no method with code, as a cache entry records it or
/// the lookup resolves it, for `stub_entry`.
pub(crate) enum Stubbed<'method> {
    /// A method with a fast path, or a `doesNotUnderstand:` handler.
    Method {
        method: &'method MixinMethod,
        chain: crate::interpreter::resolve::Chain,
    },
    Slot {
        kind: crate::mixin::AccessorKind,
        chain: crate::interpreter::resolve::Chain,
        field: usize,
    },
    Primitive {
        primitive: crate::builtins::primitive::Primitive,
        chain: crate::interpreter::resolve::Chain,
    },
}

/// The stub that does what `Mutator::act_with_base` does for `stubbed`
/// (`targets::assemble_accessor_stubs`; `docs/design/execution.md`,
/// "Sends in machine code"), chosen from it alone: a getter on a public
/// or protected chain, whose guard is the receiver's own, through its
/// field's getter stub; a quick method's constant, receiver or parameter
/// through its stub, read off the method, which is immutable, as `act`
/// reads it on every hit; a setter, a private or `super` slot accessor,
/// a builtin class's accessor, and a primitive method or a mirror send of
/// a leaf-safe row (`MachineCall::Leaf`) through the leaf stub. `None` for
/// every other -- a fast path that activates a block, a row that is not
/// leaf-safe, a `doesNotUnderstand:` handler -- and where there is no
/// zone: the interpreter runs the send.
pub(crate) fn stub_entry(state: &JitState, stubbed: &Stubbed<'_>) -> Option<u64> {
    use crate::builtins::primitive::{mirror_entry, MachineCall, Primitive};
    use crate::interpreter::resolve::Chain;
    use crate::mixin::{AccessorKind, MethodFastPath, QuickConstant};
    let zone = state.zone.as_ref()?;
    let address = |offset: CodeOffset| zone.writer.zone().executable_address(offset) as u64;
    let act = address(zone.accessors.act);
    let handler = |chain: Chain| {
        matches!(
            chain,
            Chain::DoesNotUnderstand | Chain::SuperDoesNotUnderstand
        )
    };
    let leaf_safe = |row| mirror_entry(row).machine_call == MachineCall::Leaf;
    match stubbed {
        Stubbed::Method { chain, .. }
        | Stubbed::Slot { chain, .. }
        | Stubbed::Primitive { chain, .. }
            if handler(*chain) =>
        {
            None
        }
        Stubbed::Method { method, .. } => match method.fast_path {
            Some(MethodFastPath::Constant(constant)) => Some(address(match constant {
                QuickConstant::Nil => zone.accessors.nil,
                QuickConstant::True => zone.accessors.true_constant,
                QuickConstant::False => zone.accessors.false_constant,
            })),
            Some(MethodFastPath::Receiver) => Some(address(zone.accessors.receiver)),
            // A position past the send's arguments activates the method,
            // which no compiled send of the method's selector reaches.
            Some(MethodFastPath::Parameter(position))
                if usize::from(position) < method.argument_count =>
            {
                Some(
                    zone.accessors
                        .parameters
                        .get(usize::from(position))
                        .map_or(act, |entry| address(*entry)),
                )
            }
            Some(MethodFastPath::BuiltinClass(_)) => Some(act),
            Some(MethodFastPath::Mirror(row)) if leaf_safe(row) => Some(act),
            _ => None,
        },
        Stubbed::Slot {
            kind: AccessorKind::Getter,
            chain: Chain::Public | Chain::Protected,
            field,
        } => Some(
            zone.accessors
                .getters
                .get(*field)
                .map_or(act, |entry| address(*entry)),
        ),
        Stubbed::Slot { .. } => Some(act),
        Stubbed::Primitive {
            primitive: Primitive::Mirror(row),
            ..
        } if leaf_safe(*row) => Some(act),
        Stubbed::Primitive { .. } => None,
    }
}

/// Whether machine code runs `method`, found on `chain`, as an activation
/// of its body: exactly when `Mutator::act_with_base` pushes one for it --
/// no fast path, no `doesNotUnderstand:` handler -- and its body takes the
/// arguments its selector's send hands it, so that the call lays it on
/// the send's slots as the interpreter's push does.
pub(crate) fn runs_in_machine_code(
    method: &MixinMethod,
    chain: crate::interpreter::resolve::Chain,
) -> bool {
    use crate::interpreter::resolve::Chain;
    matches!(
        chain,
        Chain::Public | Chain::Protected | Chain::Private | Chain::Super
    ) && method.fast_path.is_none()
        && method.argument_count == usize::from(method.code.argument_count)
}

impl JitState {
    /// The address of the stub a send whose guard is not its cell's
    /// calls, the polymorphic stub; 0 where there is no zone.
    pub(crate) fn polymorphic_stub_address(&self) -> u64 {
        self.zone.as_ref().map_or(0, |zone| {
            zone.writer.zone().executable_address(zone.polymorphic_stub) as u64
        })
    }

    /// The address of the miss stub, 0 where there is no zone.
    pub(crate) fn miss_stub_address(&self) -> u64 {
        self.zone.as_ref().map_or(0, |zone| {
            zone.writer.zone().executable_address(zone.miss_stub) as u64
        })
    }

    /// The installed body `code`'s slot names, when it is compiled into
    /// this interpreter's zone and the JIT is on.
    fn installed(&self, code: &CompiledCode) -> Option<&InstalledBody> {
        let slot = &code.jit;
        let compiled =
            self.enabled && slot.state.get() == COMPILED && slot.zone.get() == self.zone_identity();
        compiled
            .then(|| self.bodies.get(slot.map.get() as usize))
            .flatten()
    }

    /// Where a compiled call enters `code`'s machine code -- its prologue,
    /// the body's first byte -- when it has machine code in this zone
    /// compiled against `feedback_base`.
    fn machine_entry_of(
        &self,
        code: &CompiledCode,
        feedback_base: FeedbackBase,
    ) -> Option<CodeOffset> {
        let body = self.installed(code)?;
        (body.feedback_base == feedback_base).then_some(CodeOffset(body.start))
    }

    /// Where the run loop enters `code`'s machine code at the instruction
    /// at `program_counter`, for an activation with `feedback_base`.
    fn interpreter_entry(
        &self,
        code: &CompiledCode,
        feedback_base: FeedbackBase,
        program_counter: u32,
    ) -> Option<CodeOffset> {
        let body = self.installed(code)?;
        let offset = body.map.machine_offset(program_counter)?;
        (body.feedback_base == feedback_base).then_some(CodeOffset(body.start + offset))
    }

    /// Whether `code` may still be compiled into this zone: it is counting
    /// its entries, or compiled into another zone.
    fn may_compile(&self, code: &CompiledCode) -> bool {
        self.enabled && code.jit.state.get() <= COMPILED
    }

    /// Whether a cell's entry address `found` is one a cell of `expected`
    /// may still hold: the same, or the miss stub's, which the next miss
    /// at its site rewrites once its body is compiled or refused
    /// (`inline_cache::decide_at`).
    pub(crate) fn cell_entry_may_be(&self, found: u64, expected: u64) -> bool {
        found == expected || (found != 0 && found == self.miss_stub_address())
    }
}

/// Where `code`'s machine code starts, when it is compiled into this
/// interpreter's zone and the JIT is on: a body compiled by another
/// interpreter, or into a zone since replaced, names a zone that is not
/// this one, and its offset means nothing here.
#[cfg(test)]
pub(crate) fn machine_entry(state: &JitState, code: &CompiledCode) -> Option<CodeOffset> {
    state.installed(code).map(|body| CodeOffset(body.start))
}

/// The address a compiled call enters `code` at, its machine entry, when
/// it has machine code in this zone compiled against `feedback_base`.
pub(crate) fn machine_entry_address(
    state: &JitState,
    code: &CompiledCode,
    feedback_base: FeedbackBase,
) -> Option<u64> {
    let entry = state.machine_entry_of(code, feedback_base)?;
    let zone = state.zone.as_ref()?;
    Some(zone.writer.zone().executable_address(entry) as u64)
}

/// Counts an entry into `code`, an activation with `feedback_base` of
/// `method`'s body when it is one's; at its `COMPILE_AT_ENTRY`th, compiles
/// it. While the JIT is off, refuses it at once (`JitState::set_enabled`).
/// A body that cannot be compiled is refused for good. A body compiled
/// into a zone that is not this interpreter's -- another interpreter's, or
/// one since replaced -- is compiled again into this one.
pub(crate) fn note_entry(
    state: &mut JitState,
    code: &Rc<CompiledCode>,
    feedback_base: FeedbackBase,
    method: Option<&Rc<MixinMethod>>,
) {
    let slot = &code.jit;
    if !state.enabled {
        slot.refuse();
        state.statistics.refused_bodies += 1;
        return;
    }
    match slot.state.get() {
        REFUSED | EXITS_AT_ONCE => {}
        COMPILED if slot.zone.get() == state.zone_identity() => {}
        // A body two interpreters ran in turn would be compiled again at
        // every switch, each time leaving its old code in the other's
        // zone; no body is run by two interpreters today
        // (`docs/OPEN-WORK.md`, road step 8).
        COMPILED => compile_into_slot(state, code, feedback_base, method),
        entries => {
            let entries = entries + 1;
            if entries >= state.compile_at_entry {
                compile_into_slot(state, code, feedback_base, method);
            } else {
                slot.state.set(entries);
            }
        }
    }
}

/// `note_entry` for a compiled call that found no machine code for its
/// callee (`inline_cache::decide_at`): the entry is counted only when it is
/// the one that compiles the body -- its `COMPILE_AT_ENTRY`th, or a body
/// compiled into another zone -- since the call is then made in machine
/// code. Otherwise nothing is counted: the call is refused, and the
/// interpreter's push of the callee, running the send again, counts it, so
/// that each push counts once.
pub(crate) fn note_entry_by_call(
    state: &mut JitState,
    code: &Rc<CompiledCode>,
    feedback_base: FeedbackBase,
    method: Option<&Rc<MixinMethod>>,
) {
    let compiles = state.enabled
        && match code.jit.state.get() {
            REFUSED | EXITS_AT_ONCE => false,
            COMPILED => code.jit.zone.get() != state.zone_identity(),
            entries => entries + 1 >= state.compile_at_entry,
        };
    if compiles {
        note_entry(state, code, feedback_base, method);
    }
}

/// Compiles `code` into the zone and records the outcome in its slot and
/// the statistics.
fn compile_into_slot(
    state: &mut JitState,
    code: &Rc<CompiledCode>,
    feedback_base: FeedbackBase,
    method: Option<&Rc<MixinMethod>>,
) {
    match compile(state, code, feedback_base, method) {
        Ok((offset, map)) => {
            code.jit.compiled(state.zone_identity(), offset, map);
            state.statistics.compiled_bodies += 1;
        }
        Err(Refusal::ExitsAtOnce) => {
            code.jit.settle(EXITS_AT_ONCE);
            state.statistics.bodies_exiting_at_once += 1;
        }
        Err(_) => {
            code.jit.refuse();
            state.statistics.refused_bodies += 1;
        }
    }
}

/// Compiles `code`, its sends' cells those of `feedback_base`'s range,
/// and installs it, answering its offset and its index among the
/// installed bodies.
#[cfg(ns_jit)]
fn compile(
    state: &mut JitState,
    code: &Rc<CompiledCode>,
    feedback_base: FeedbackBase,
    method: Option<&Rc<MixinMethod>>,
) -> Result<(CodeOffset, u32), Refusal> {
    let Some(zone) = state.zone.as_mut() else {
        panic!("internal: the JIT is on without a zone");
    };
    let mut macro_assembler = macro_assembler::MacroAssembler::new(macro_assembler::Abi::host());
    macro_assembler.set_count_back_edges(state.counting);
    let body = baseline::compile(code, macro_assembler, feedback_base)?;
    if body.exits_at_once && !state.install_bodies_exiting_at_once {
        return Err(Refusal::ExitsAtOnce);
    }
    let map = u32::try_from(state.bodies.len()).map_err(|_| Refusal::ZoneFull)?;
    let offset = install(zone, &body.code.bytes)?;
    let length = u32::try_from(body.code.bytes.len()).map_err(|_| Refusal::ZoneFull)?;
    state.bodies.push(InstalledBody {
        code: Rc::downgrade(code),
        method: method.map(Rc::downgrade),
        feedback_base,
        start: offset.0,
        end: offset.0 + length,
        map: body.map,
    });
    Ok((offset, map))
}

#[cfg(not(ns_jit))]
fn compile(
    _state: &mut JitState,
    _code: &Rc<CompiledCode>,
    _feedback_base: FeedbackBase,
    _method: Option<&Rc<MixinMethod>>,
) -> Result<(CodeOffset, u32), Refusal> {
    Err(Refusal::Unassembled)
}

/// Where the run loop goes on after it entered machine code.
pub(crate) enum Resumption {
    /// Interpreted: `activation`, the frame the run loop entered or the
    /// innermost one an exit materialized above it, at this program
    /// counter -- the entry's own when no machine code ran.
    Interpreted {
        activation: ActivationReference,
        program_counter: u32,
    },
    /// Machine code found a safepoint requested before the instruction at
    /// this program counter of `activation`; the loop takes it there, as a
    /// backward jump that found one does.
    Safepoint {
        activation: ActivationReference,
        program_counter: u32,
    },
    /// The frame the run loop entered returned the value whose word this
    /// is: the loop pops it and applies its continuation, as for a return
    /// it ran itself (`vm::finish`).
    Returned { value: u64 },
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
    code: &Rc<CompiledCode>,
    register_base: RegisterBase,
) -> Resumption {
    let feedback_base = mutator.heap.activation_feedback_base(activation);
    let method = mutator.heap.activation_method(activation);
    note_entry(&mut mutator.jit, code, feedback_base, method.as_ref());
    match mutator.jit.interpreter_entry(code, feedback_base, 0) {
        Some(entry) => run_machine_code(mutator, activation, register_base, entry),
        None => Resumption::Interpreted {
            activation,
            program_counter: 0,
        },
    }
}

/// A frame returned to, which an exit above it left stopped in machine
/// code (`Heap::activation_is_compiled`), its callee's value delivered:
/// runs its machine code from `program_counter`, the instruction after the
/// send, when its body has machine code in this zone, or clears the bit
/// and goes on interpreted.
#[cold]
#[inline(never)]
pub(crate) fn resumed(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    code: &Rc<CompiledCode>,
    register_base: RegisterBase,
    program_counter: u32,
) -> Resumption {
    let feedback_base = mutator.heap.activation_feedback_base(activation);
    match mutator
        .jit
        .interpreter_entry(code, feedback_base, program_counter)
    {
        Some(entry) if native_stack_has_room() => {
            mutator.jit.statistics.resumed_entries += 1;
            run_machine_code(mutator, activation, register_base, entry)
        }
        _ => {
            mutator.heap.clear_compiled(activation);
            Resumption::Interpreted {
                activation,
                program_counter,
            }
        }
    }
}

/// Runs the machine code at `entry` on `activation`'s frame, whose
/// registers start at `register_base`, and answers where the interpreter
/// goes on: at 0, running nothing, when the thread's native stack has less
/// than `NATIVE_STACK_NEED` left, since compiled frames' return addresses
/// lie there. Every address the code needs is asked of the heap afresh, as
/// nothing keeps one valid across entries.
///
/// # Panics
///
/// On a debug build's read of a dead register, with the interpreter's own
/// refusal of one (`Heap::refuse_dead_register`).
#[cfg(ns_jit)]
fn run_machine_code(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    register_base: RegisterBase,
    entry: CodeOffset,
) -> Resumption {
    use ns_memory::ENTRY_BLOCK_WORDS;
    use stubs::{BlockWord, MachineExit};

    if !native_stack_has_room() {
        return Resumption::Interpreted {
            activation,
            program_counter: 0,
        };
    }
    let Some(zone) = mutator.jit.zone.as_ref() else {
        panic!("internal: machine code is entered without a zone");
    };
    // The zone is held apart from the mutator, which `enter` lends to the
    // code as its context.
    let code = std::rc::Rc::clone(zone.writer.zone());
    let (entry_stub, exit_stub) = (zone.entry_stub, zone.exit_stub);
    let mut block = [0u64; ENTRY_BLOCK_WORDS];
    block[BlockWord::ObjectSpaceBase.index()] = mutator.heap.object_space_base_address() as u64;
    block[BlockWord::StackBase.index()] = mutator.heap.activation_stack_base_address() as u64;
    block[BlockWord::RegisterBase.index()] = crate::heap::Heap::register_base_words(register_base);
    block[BlockWord::MachineEntry.index()] = code.executable_address(entry) as u64;
    block[BlockWord::MachineWords.index()] = mutator.heap.machine_words_address() as u64;
    block[BlockWord::ExitStub.index()] = code.executable_address(exit_stub) as u64;
    block[BlockWord::Cells.index()] = mutator.caches.site_cells_address() as u64;
    block[BlockWord::PolymorphicStub.index()] = mutator.jit.polymorphic_stub_address();
    block[BlockWord::PolymorphicBlocks.index()] = mutator.caches.block_cells_address() as u64;
    block[BlockWord::GlobalRows.index()] = mutator.caches.global_rows_address() as u64;
    block[BlockWord::GlobalMask.index()] = mutator.caches.global_mask();
    let leaves = leaves::table();
    mutator.jit.statistics.machine_entries += 1;
    mutator.jit.walked = None;
    let kind = code.enter(entry_stub, &mut block, mutator, &leaves);
    debug_assert_eq!(
        kind,
        block[BlockWord::ExitKind.index()],
        "internal: the exit stub answers the kind it wrote"
    );
    if mutator.jit.counting {
        mutator.jit.statistics.machine_back_edges += block[BlockWord::BackEdges.index()];
        mutator.jit.statistics.compiled_calls += block[BlockWord::CompiledCalls.index()];
    }
    // The innermost frame the exit materialized, or the entry's when no
    // compiled frame lay above it.
    let innermost = mutator.jit.take_walked().unwrap_or(activation);
    let exit = stubs::decode_exit(&block, kind);
    let at_a_send = match exit {
        MachineExit::PushRefused => true,
        MachineExit::Reexecute { program_counter } => matches!(
            mutator
                .heap
                .activation_code(innermost)
                .instruction(program_counter as usize),
            crate::bytecode::Instruction::SendPublic { .. }
                | crate::bytecode::Instruction::SendLexical { .. }
                | crate::bytecode::Instruction::SendSuper { .. }
                | crate::bytecode::Instruction::SendInlinable { .. }
                | crate::bytecode::Instruction::SendMirror { .. }
        ),
        _ => false,
    };
    if at_a_send {
        mutator.jit.statistics.send_exits += 1;
    }
    // The innermost frame goes on interpreted from here: it no longer
    // stands in machine code, which only the frames below it still do.
    if !matches!(exit, MachineExit::Returned { .. }) {
        mutator.heap.clear_compiled(innermost);
    }
    match exit {
        MachineExit::Returned { value } => Resumption::Returned { value },
        MachineExit::Reexecute { program_counter } => Resumption::Interpreted {
            activation: innermost,
            program_counter,
        },
        // The innermost frame is the caller, stopped at the send the walk
        // wrote as its program counter: it runs the send again.
        MachineExit::PushRefused => {
            let program_counter = mutator.heap.activation_program_counter(innermost);
            Resumption::Interpreted {
                activation: innermost,
                program_counter: u32::try_from(program_counter)
                    .expect("internal: a program counter fits 32 bits"),
            }
        }
        MachineExit::Safepoint { program_counter } => {
            mutator.jit.statistics.safepoint_exits += 1;
            Resumption::Safepoint {
                activation: innermost,
                program_counter,
            }
        }
        MachineExit::DeadRegister { register, .. } => {
            mutator.heap.refuse_dead_register(innermost, register)
        }
    }
}

#[cfg(not(ns_jit))]
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
    use super::{CallReturn, JitSlot, ProgramCounterMap};

    #[test]
    fn the_jit_is_available_on_x64() {
        assert!(!cfg!(target_arch = "x86_64") || super::available());
    }

    #[test]
    fn a_program_counter_map_answers_both_ways() {
        let map = ProgramCounterMap::new(vec![0, 16, 16, 40], Vec::new());
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
    fn a_return_address_maps_to_the_send_whose_call_returns_there() {
        let map = ProgramCounterMap::new(
            vec![0, 16, 40, 64],
            vec![
                CallReturn {
                    machine_offset: 29,
                    program_counter: 1,
                },
                CallReturn {
                    machine_offset: 52,
                    program_counter: 2,
                },
            ],
        );
        assert_eq!(map.program_counter_of_return(29), Some(1));
        assert_eq!(map.program_counter_of_return(52), Some(2));
        assert_eq!(
            map.program_counter_of_return(16),
            None,
            "a start, not a return"
        );
        assert_eq!(map.program_counter_of_return(30), None);
        assert_eq!(map.program_counter_of_return(u32::MAX), None);
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

    /// The machine-code tests, where the build emits machine code.
    #[cfg(ns_jit)]
    mod machine {
        use std::rc::Rc;

        use ns_memory::Leaf;

        use crate::bytecode::{CodeKind, CompiledCode, Instruction, Literal};
        use crate::heap::Immediate;
        use crate::interpreter::FeedbackBase;
        use crate::interpreter::Interpreter;
        use crate::jit::macro_assembler::{Abi, LeafIndex, MacroAssembler, Operand};
        use crate::jit::stubs::testing::run_in;
        use crate::jit::stubs::{ExitKind, MachineExit};
        use crate::jit::{install, machine_entry, note_entry, JitState};

        fn body() -> Rc<CompiledCode> {
            Rc::new(CompiledCode::new(
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
            ))
        }

        /// A body whose first instruction, an error the interpreter raises,
        /// is an exit.
        fn body_exiting_at_once() -> Rc<CompiledCode> {
            Rc::new(CompiledCode::new(
                CodeKind::Method,
                vec![Instruction::SignalNonBoolean { register: 13 }],
                0,
                14,
                0,
                Vec::new(),
                ns_syntax::Span::new(0, 0),
            ))
        }

        /// A body whose template is followed by an exit.
        fn body_exiting_after_a_template() -> Rc<CompiledCode> {
            Rc::new(CompiledCode::new(
                CodeKind::Method,
                vec![
                    Instruction::LoadLiteral {
                        destination: 13,
                        literal: Literal::SmallInteger(3),
                    },
                    Instruction::SignalNonBoolean { register: 13 },
                ],
                0,
                14,
                0,
                Vec::new(),
                ns_syntax::Span::new(0, 0),
            ))
        }

        /// A body whose templates jump back before its return: a loop that
        /// runs while its register holds `true`, which it never does.
        fn body_with_a_loop() -> Rc<CompiledCode> {
            Rc::new(CompiledCode::new(
                CodeKind::Method,
                vec![
                    Instruction::LoadLiteral {
                        destination: 13,
                        literal: Literal::Boolean(false),
                    },
                    Instruction::BranchIfTrue {
                        register: 13,
                        target: 0,
                    },
                    Instruction::Return { register: 13 },
                ],
                0,
                14,
                0,
                Vec::new(),
                ns_syntax::Span::new(0, 0),
            ))
        }

        fn enter_twice(state: &mut JitState, code: &Rc<CompiledCode>) {
            note_entry(state, code, FeedbackBase::NOT_ASSIGNED, None);
            note_entry(state, code, FeedbackBase::NOT_ASSIGNED, None);
        }

        #[test]
        fn a_body_that_runs_to_a_return_is_compiled() {
            let mut state = JitState::for_test(1 << 16, false);
            let code = body();
            enter_twice(&mut state, &code);
            assert!(machine_entry(&state, &code).is_some());
            assert_eq!(state.statistics().compiled_bodies, 1);
            assert_eq!(state.statistics().bodies_exiting_at_once, 0);
        }

        #[test]
        fn a_body_exiting_at_once_is_never_entered() {
            let mut state = JitState::for_test(1 << 16, false);
            let exiting_at_once = body_exiting_at_once();
            let exiting_after_a_template = body_exiting_after_a_template();
            for code in [&exiting_at_once, &exiting_after_a_template] {
                enter_twice(&mut state, code);
                assert_eq!(format!("{:?}", code.jit), "JitSlot::ExitsAtOnce");
                assert!(!code.jit.is_armed(), "a push leaves it alone");
                assert_eq!(machine_entry(&state, code), None);
            }
            let statistics = state.statistics();
            assert_eq!(statistics.bodies_exiting_at_once, 2);
            assert_eq!(statistics.compiled_bodies, 0);
            assert_eq!(statistics.refused_bodies, 0);
        }

        #[test]
        fn a_body_that_can_jump_back_before_it_leaves_is_compiled() {
            let mut state = JitState::for_test(1 << 16, false);
            let code = body_with_a_loop();
            enter_twice(&mut state, &code);
            assert!(machine_entry(&state, &code).is_some());
            assert_eq!(state.statistics().compiled_bodies, 1);
            assert_eq!(state.statistics().bodies_exiting_at_once, 0);
        }

        #[test]
        fn a_disabled_jit_refuses_a_body_at_its_first_entry() {
            let mut state = JitState::for_test(1 << 16, true);
            let code = body();
            let _ = state.set_enabled(false);
            note_entry(&mut state, &code, FeedbackBase::NOT_ASSIGNED, None);
            assert_eq!(format!("{:?}", code.jit), "JitSlot::Refused");
            assert!(!code.jit.is_armed(), "a later push pays only the compare");
            assert_eq!(state.statistics().refused_bodies, 1);
            let _ = state.set_enabled(true);
            note_entry(&mut state, &code, FeedbackBase::NOT_ASSIGNED, None);
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
            note_entry(&mut second, &code, FeedbackBase::NOT_ASSIGNED, None);
            assert!(machine_entry(&second, &code).is_some());
            assert_eq!(machine_entry(&first, &code), None);
            assert_eq!(second.statistics().compiled_bodies, 1);
        }

        /// What the leaf that compiles a body while machine code runs is
        /// handed: the JIT's state, as a miss leaf reaches it through the
        /// mutator, and the body it compiles.
        struct Compiling {
            state: JitState,
            code: Rc<CompiledCode>,
        }

        fn compile_the_body(context: &mut Compiling, _arguments: [u64; 3]) -> u64 {
            enter_twice(&mut context.state, &context.code);
            0
        }

        #[test]
        fn a_leaf_can_install_while_machine_code_runs() {
            let mut context = Compiling {
                state: JitState::for_test(1 << 16, false),
                code: body_with_a_loop(),
            };
            let mut macro_assembler = MacroAssembler::new(Abi::host());
            macro_assembler.call_leaf(LeafIndex(0), [Operand::Immediate(0); 3]);
            macro_assembler.exit(ExitKind::Reexecute, 0);
            let caller = macro_assembler.finish().expect("assembled");
            let zone = context.state.zone.as_mut().expect("a zone");
            let caller = install(zone, &caller.bytes).expect("installed");
            let stubs = [zone.entry_stub, zone.exit_stub];
            let executable = Rc::clone(zone.writer.zone());
            let mut frame = [0u64; 16];
            let leaves: [Leaf<Compiling>; 1] = [compile_the_body];
            let (exit, _) = run_in(
                &executable,
                stubs,
                caller,
                &mut frame,
                0,
                false,
                &mut context,
                &leaves,
            );
            assert_eq!(exit, MachineExit::Reexecute { program_counter: 0 });
            let entry = context
                .state
                .interpreter_entry(&context.code, FeedbackBase::NOT_ASSIGNED, 0)
                .expect("compiled and installed while the caller ran");
            // Its next push enters it: the loop's test fails, and its
            // return hands the value back to the entry stub.
            let (exit, _) = run_in(
                &executable,
                stubs,
                entry,
                &mut frame,
                0,
                false,
                &mut (),
                &[],
            );
            assert_eq!(
                exit,
                MachineExit::Returned {
                    value: Immediate::boolean(false).bits()
                }
            );
            assert_eq!(frame[13], Immediate::boolean(false).bits());
        }

        #[test]
        fn every_return_address_into_an_installed_body_names_it() {
            let mut interpreter = Interpreter::bootstrap_with(crate::Lowering::Optimized);
            interpreter.enter(|mutator| every_return_address_names_its_body(&mut mutator.heap));
        }

        fn every_return_address_names_its_body(heap: &mut crate::heap::Heap<'_>) {
            let mut state = JitState::for_test(1 << 16, false);
            let first = body_with_a_loop();
            let second = body_with_a_loop();
            enter_twice(&mut state, &first);
            enter_twice(&mut state, &second);
            let third = body_with_a_loop();
            enter_twice(&mut state, &third);
            let zone = state.zone.as_ref().expect("a zone");
            let address = |offset| zone.writer.zone().executable_address(offset);
            let first_start = address(machine_entry(&state, &first).expect("compiled"));
            let second_start = address(machine_entry(&state, &second).expect("compiled"));
            let third_start = address(machine_entry(&state, &third).expect("compiled"));
            let length =
                |index: usize| (state.bodies[index].end - state.bodies[index].start) as usize;
            for (start, end, code) in [
                (first_start, first_start + length(0), &first),
                (second_start, second_start + length(1), &second),
            ] {
                // A return address lies past its call, so one at the body's
                // very end still names it, and one at its start does not.
                for return_address in [start + 1, (start + end) / 2, end] {
                    let (code_id, map) = state.body_at(return_address, heap).expect("in a body");
                    let named = heap.code(code_id).expect("a live row");
                    assert!(Rc::ptr_eq(&named, code), "its own body");
                    assert!(
                        map.machine_offset(0).is_some_and(|start| start > 0),
                        "past the prologue"
                    );
                    assert_eq!(state.body_start(return_address), Some(start));
                }
            }
            assert!(
                state.body_at(first_start, heap).is_none(),
                "the stubs' code, or the padding after them"
            );
            assert!(state
                .body_at(address(ns_memory::CodeOffset(1)), heap)
                .is_none());
            assert!(state.body_at(0, heap).is_none(), "outside the zone");
            // A body that died answers nothing, never another body. (The
            // two above live on in the heap's code rows, which `body_at`
            // registered them in, until a full collection.)
            drop(third);
            assert!(state.body_at(third_start + 1, heap).is_none());
            assert!(state.body_at(second_start + 1, heap).is_some());
        }

        #[test]
        fn a_full_zone_refuses_bodies() {
            let mut state = JitState::for_test(4096, true);
            let bodies: Vec<Rc<CompiledCode>> = (0..200).map(|_| body()).collect();
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
