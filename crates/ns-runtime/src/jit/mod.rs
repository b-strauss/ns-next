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
pub(crate) mod closures;
#[cfg(ns_jit)]
pub(crate) mod cut;
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
pub(crate) mod zone;

use std::cell::Cell;
use std::rc::{Rc, Weak};

use ns_memory::{CodeOffset, CodeWriter, EntryBlock};

use crate::bytecode::CompiledCode;
use crate::hash::IdentityMap;
use crate::heap::{ActivationReference, ClassReference, CodeId, Heap, RegisterBase, Value};
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
    /// compiler: too many instructions, or code the assembler could not
    /// lay out -- which stay interpreted for good. A body that found no
    /// room in the zone is not refused: it counts its entries again, and
    /// the zone is compacted (`jit::zone`).
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
    /// Bodies the JIT abandoned once their machine code had handed their
    /// frames to the interpreter too often (`EXIT_BUDGET`): neither entered
    /// nor called by any cell until their exits decay
    /// (`revived_bodies`). A body abandoned again after a revival counts
    /// again.
    pub abandoned_bodies: u64,
    /// Calls of the leaf stub's leaf (`jit::leaves`'s `act`), which runs a
    /// target that is no method with code -- a setter, a leaf-safe
    /// primitive -- as the interpreter's `act` does.
    pub act_leaf_calls: u64,
    /// Calls of the leaf stub's leaf that ran a slot accessor straight
    /// from the entry of the send's own site, whose cell the send hit, and
    /// answered its value; a decline, which refuses the call, is not
    /// counted.
    pub site_slot_hits: u64,
    /// Entries into machine code at an interpreter's send, which called a
    /// compiled callee's machine entry through the call stub (`called`),
    /// whatever the callee's checks then did.
    pub called_entries: u64,
    /// Exits of machine code for any reason but a return -- an
    /// instruction re-executed, a safepoint, a refused push, a dead
    /// register's read -- each handing a frame to the interpreter.
    pub exits: u64,
    /// Calls machine code made of a leaf (`jit::leaves`), of every kind:
    /// a template's, a stub's and the exit's walk's.
    pub leaf_calls: u64,
    /// Non-local returns machine code carried out itself, popping its
    /// compiled frames down to the home and returning from it there
    /// (`jit::cut`), rather than exiting so that the interpreter's
    /// unwinder carries them.
    pub cuts: u64,
    /// Compactions of the code zone (`jit::zone`): at a safepoint after a
    /// compile found no room, an exhausted zone held enough compiles, or a
    /// full collection freed a quarter of the zone; or the zone stress
    /// mode's.
    pub zone_compactions: u64,
    /// Bodies whose machine code was freed: dead or no longer named by
    /// their slots, at a full collection or a compaction; abandoned; or
    /// freed by their usage. Counted when freed, before a compaction
    /// reclaims their bytes.
    pub freed_bodies: u64,
    /// Bytes of machine code and data words the compactions slid down.
    pub moved_bytes: u64,
    /// Abandoned bodies whose exit count the scavenges and full
    /// collections had halved below `REVIVAL_EXITS`, which count their
    /// entries again (`JitState::decay_exits`).
    pub revived_bodies: u64,
}

/// What a body's `JitSlot` says once it is compiled, and so entered at a
/// send, a push and a frame returned to.
const COMPILED: u32 = u32::MAX - 3;
/// What a body's `JitSlot` says once the JIT abandoned it for its exits
/// (`abandon`): neither compiled nor entered until the scavenges and full
/// collections have halved its exit count below `REVIVAL_EXITS`
/// (`JitState::decay_exits`). Above `COMPILED`, as every state that is not
/// armed is, so that `is_armed` stays one compare.
const ABANDONED: u32 = u32::MAX - 2;
/// What a body's `JitSlot` says once its machine code is known to leave
/// before it reaches a return, a compiled send or a backward jump, so that
/// it is never entered.
const EXITS_AT_ONCE: u32 = u32::MAX - 1;
/// What a body's `JitSlot` says once it will never be compiled.
const REFUSED: u32 = u32::MAX;

/// How many times a body's machine code may hand its frame to the
/// interpreter -- an exit at an instruction or a refused call, not a
/// poll -- before the JIT abandons it (`abandon`): it is neither compiled
/// nor entered, so that neither a push nor a frame returned to enters it,
/// and every cell that called it refuses the call instead, so that its
/// callers, refused in turn, spend their own budgets. An exit and the
/// entry that resumes the frame cost about as much as a hundred
/// interpreted instructions, so a loop that leaves on every turn -- one
/// that sends `ifTrue:` a block in a variable, which machine code leaves
/// to the interpreter, or calls a body that does -- ran more slowly in
/// machine code than interpreted; a region of bodies that keep leaving
/// ends up interpreted, and one that does not stays in machine code.
///
/// The count decays: every scavenge and full collection halves it
/// (`JitState::decay_exits`), so that the budget bounds the exits of a
/// stretch of the run rather than of its lifetime, and a rare exit does
/// not end a long run interpreted.
const EXIT_BUDGET: u32 = 64;

/// The exit count below which an abandoned body is revived: it counts its
/// entries again, and is compiled at its `COMPILE_AT_ENTRY`th. Half the
/// budget, so that a body abandoned at `EXIT_BUDGET` needs two halvings --
/// two scavenges or full collections -- and a body that always exits is
/// compiled at most once per two of them.
const REVIVAL_EXITS: u32 = EXIT_BUDGET / 2;

/// A body's state in the JIT: *counting* its entries (`state` below
/// `COMPILED`), *compiled* (`COMPILED`; `zone` names the zone its machine
/// code lies in, `offset` where, `map` its program-counter map), *exiting
/// at once* (`EXITS_AT_ONCE`: its code would exit before it reaches a
/// return, a compiled send or a backward jump, so entering it would cost
/// every push more than the few instructions it runs), *abandoned*
/// (`ABANDONED`, until its exits decay) or *refused* (`REFUSED`). A push
/// looks at a body in none of the last three, so they are the top values,
/// which one compare tells from the others (`is_armed`).
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
    /// Where the code of the body's instruction at program counter 0
    /// starts, an offset into the zone: where an entry at a push goes in,
    /// recorded so that no entry searches the program-counter map for it.
    first_instruction: Cell<u32>,
    map: Cell<u32>,
    /// How many times the body's machine code handed its frame to the
    /// interpreter (`EXIT_BUDGET`).
    exits: Cell<u32>,
    /// The bytes the body's machine code needed, its alignment's slack
    /// included, when a compile last found no room for it; 0 if none
    /// did. While the zone is exhausted, a body that came due compiles
    /// only if the room left holds this much (`JitState::may_compile_now`).
    needed: Cell<u32>,
}

impl Default for JitSlot {
    fn default() -> JitSlot {
        JitSlot {
            state: Cell::new(0),
            zone: Cell::new(0),
            offset: Cell::new(0),
            first_instruction: Cell::new(0),
            map: Cell::new(0),
            exits: Cell::new(0),
            needed: Cell::new(0),
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
            ABANDONED => write!(formatter, "JitSlot::Abandoned({})", self.exits.get()),
            EXITS_AT_ONCE => formatter.write_str("JitSlot::ExitsAtOnce"),
            COMPILED => formatter
                .debug_struct("JitSlot::Compiled")
                .field("zone", &self.zone.get())
                .field("offset", &self.offset.get())
                .field("first_instruction", &self.first_instruction.get())
                .field("map", &self.map.get())
                .finish(),
            entries => write!(formatter, "JitSlot::Counting({entries})"),
        }
    }
}

impl JitSlot {
    /// Whether an entry into the body has anything to do with the JIT:
    /// false once the body is refused, abandoned or exits at once. The one
    /// load and
    /// compare the run loop makes on a push (`vm::run_stretch`);
    /// everything else is `entered`'s, out of line.
    #[inline(always)]
    pub(crate) fn is_armed(&self) -> bool {
        self.state.get() <= COMPILED
    }

    /// Whether the body is compiled, whether or not into this
    /// interpreter's zone: the compare an interpreter's send of a method
    /// makes, after the loads that reach the slot from the method, before
    /// it calls the body's machine code (`Mutator::enter_sent`);
    /// everything else is `called`'s, out of line.
    #[inline(always)]
    pub(crate) fn is_compiled(&self) -> bool {
        self.state.get() == COMPILED
    }

    fn refuse(&self) {
        self.settle(REFUSED);
    }

    /// Counts an exit of the body's machine code that handed its frame to
    /// the interpreter, answering whether the body has just spent its
    /// `EXIT_BUDGET`.
    fn note_exit(&self) -> bool {
        let exits = self.exits.get().saturating_add(1);
        self.exits.set(exits);
        exits >= EXIT_BUDGET && self.state.get() == COMPILED
    }

    /// Halves the count of the body's exits (`JitState::decay_exits`),
    /// answering it.
    fn decay_exits(&self) -> u32 {
        let exits = self.exits.get() / 2;
        self.exits.set(exits);
        exits
    }

    fn settle(&self, state: u32) {
        self.state.set(state);
        self.zone.set(0);
    }

    /// Puts the body back to counting its entries, from none, its exits
    /// forgotten with its code: a compile that found the zone full, and a
    /// compaction that freed its machine code by its usage (`jit::zone`),
    /// and the revival of an abandoned body (`JitState::decay_exits`),
    /// leave it to be compiled again at its next `COMPILE_AT_ENTRY`th
    /// entry, as Cog forgets the machine code of a method it freed.
    fn forget(&self) {
        self.settle(0);
        self.exits.set(0);
    }

    /// Records the body compiled into `zone` at `offset`, its instruction
    /// at program counter 0 at `first_instruction`, the installed body
    /// `map`.
    fn compiled(&self, zone: u64, offsets: [CodeOffset; 2], map: u32) {
        let [offset, first_instruction] = offsets;
        self.state.set(COMPILED);
        self.zone.set(zone);
        self.offset.set(offset.0);
        self.first_instruction.set(first_instruction.0);
        self.map.set(map);
        self.needed.set(0);
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
    /// call of the body returns there. The tests' reading of the map: the
    /// leaves find a return address in `JitState::returns`, filled from it.
    #[cfg(test)]
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
    /// Where the first body goes, past the stubs, which never move: where
    /// a compaction slides the survivors down to (`jit::zone`).
    bodies_start: u32,
    entry_stub: CodeOffset,
    /// The stub through which an interpreter's send calls a compiled
    /// callee (`stubs::assemble_call_stub`).
    call_stub: CodeOffset,
    /// The stub through which an interpreter's send calls a block's
    /// compiled body (`stubs::assemble_closure_call_stub`).
    closure_call_stub: CodeOffset,
    exit_stub: CodeOffset,
    /// The stub a cell names for a method whose body has no machine code
    /// yet (`stubs::assemble_miss_stub`).
    miss_stub: CodeOffset,
    /// The stub a cell names for a target machine code does not run
    /// (`stubs::assemble_refusing_stub`).
    refusing_stub: CodeOffset,
    /// The stub a `NonLocalReturn`'s template calls to cut the return
    /// (`stubs::assemble_cut_stub`).
    cut_stub: CodeOffset,
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

impl Zone {
    /// The bytes bodies may take: the zone's past its stubs.
    fn room(&self) -> u32 {
        self.limit.saturating_sub(self.bodies_start)
    }
}

/// Where each accessor stub lies in the zone
/// (`targets::assemble_accessor_stubs`).
struct AccessorStubs {
    /// The getter of each field that has one, by field.
    getters: Vec<CodeOffset>,
    /// The setter of each field that has one, by field.
    setters: Vec<CodeOffset>,
    /// The quick method answering each parameter position that has one,
    /// by position.
    parameters: Vec<CodeOffset>,
    /// The method whose fast path is each mirror row, by row.
    mirror_methods: Vec<CodeOffset>,
    /// The method whose fast path is a row with a stub of its own, by
    /// the stub's index (`RowStub::index`).
    row_stubs: Vec<CodeOffset>,
    nil: CodeOffset,
    true_constant: CodeOffset,
    false_constant: CodeOffset,
    receiver: CodeOffset,
    /// The leaf stub, which runs `act`'s arm through the leaf shim.
    act: CodeOffset,
    /// The getter of a private or `super` chain of each field that has
    /// one, by field.
    guarded_getters: Vec<CodeOffset>,
    /// The setter of a private or `super` chain of each field that has
    /// one, by field.
    guarded_setters: Vec<CodeOffset>,
    /// The nested class accessor of each hidden slot that has one, by
    /// slot.
    nested_classes: Vec<CodeOffset>,
    /// The closure stub of a block row of each arity, by arity
    /// (`targets::ClosureStub::ClosureValue`).
    closure_values: Vec<CodeOffset>,
    /// The closure stub of `^<parameter> value`, by
    /// `targets::parameter_stub_index` (`targets::ClosureStub::ValueOfParameter`).
    values_of_parameters: Vec<CodeOffset>,
    /// The closure stub of `^<parameter> cull: self`, by
    /// `targets::parameter_stub_index` (`targets::ClosureStub::CullParameter`).
    culls: Vec<CodeOffset>,
}

/// Why a body was not compiled. It stays interpreted for good, but for a
/// full zone's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// More instructions than a program counter of the map holds.
    TooManyInstructions,
    /// The assembler could not lay the body out.
    Unassembled,
    /// No room left in the zone, or its memory could not be committed:
    /// the body counts its entries again, and the zone is compacted at
    /// the next safepoint (`jit::zone`).
    ZoneFull,
    /// More bytes than the zone holds past its stubs, so that no
    /// compaction could make room for the body.
    TooLarge,
    /// The code exits before it reaches a return, a compiled send or a
    /// backward jump (`JitStatistics::bodies_exiting_at_once`).
    ExitsAtOnce,
}

/// What a call's return address says of its send (`JitState::returns`).
#[derive(Clone, Copy)]
struct ReturnSite {
    /// The installed body the call lies in, by its index in `bodies`.
    body: u32,
    /// The send's program counter.
    program_counter: u32,
    /// The send's first argument slot, on which its callee's frame lies.
    argument_base: u16,
    /// How many arguments the send has.
    argument_count: u16,
    /// The send's selector.
    selector: crate::Symbol,
}

/// What a call's return address says of its send, for the leaves a callee
/// calls (`JitState::call_site`).
#[derive(Clone, Copy)]
pub(crate) struct CallSiteOperands {
    pub(crate) program_counter: u32,
    pub(crate) argument_base: u16,
    pub(crate) argument_count: u16,
    pub(crate) selector: crate::Symbol,
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
    /// code's row (`materialize`). Neither upgrade fails while a frame runs
    /// the body -- and so while a return address into it lies on the
    /// native stack: the frame names the body's row of the heap's code
    /// table (`heap/tables.rs`'s `CodeEntry`), which holds the code and its
    /// method, and a full collection reclaims no row a frame names. The
    /// exit's walk therefore treats a failed upgrade as unreachable; a
    /// change that frees or replaces a row a frame still names must keep
    /// both alive instead.
    method: Option<Weak<MixinMethod>>,
    /// The feedback base the body's sends' cells were compiled against:
    /// its machine code is entered only by an activation with that base.
    feedback_base: FeedbackBase,
    /// Whether the body reifies its frame (`baseline::reifies_its_frame`),
    /// so that the frame keeps words its prologue wrote
    /// (`MachineFrame::reifies`).
    reifies: bool,
    /// The offsets of its first byte and of the byte past its last.
    start: u32,
    end: u32,
    /// The offsets of its first data word (`MacroAssembler::take_body_words`),
    /// past its last instruction, and of the byte past its last word, short
    /// of the padding after them; both `end` for a body that has none.
    words: u32,
    words_end: u32,
    map: ProgramCounterMap,
    /// The multiple of bytes the body starts at, its assembler's alignment
    /// or `BODY_ALIGNMENT`, whichever is the greater: a compaction keeps it
    /// there, so that its data words stay on cache lines of their own.
    alignment: u32,
    /// Cog's `cmUsageCount` (`jit::zone`): born at `zone::INITIAL_USAGE`,
    /// raised by a cell or a machine entry row written to name the body,
    /// set to `zone::MAXIMUM_USAGE` while an activation runs it, halved by
    /// every compaction; a compaction short of room frees the least used
    /// first.
    usage: Cell<u8>,
    /// Whether the body is freed: no lookup finds it, its returns are gone,
    /// and the next compaction reclaims its bytes.
    free: bool,
}

impl InstalledBody {
    /// Raises the usage of a body a cell or a machine entry row is written
    /// to name, while it is below `zone::LINKED_USAGE_CEILING`, as Cog
    /// raises a linked send's target below `CMMaxUsageCount / 2`
    /// (`incrementUsageOfTargetIfLinkedSend`, `cogitX64SysV.c:8697-8698`):
    /// only the active bodies reach the maximum.
    fn note_linked(&self) {
        let usage = self.usage.get();
        if usage < zone::LINKED_USAGE_CEILING {
            self.usage.set(usage + 1);
        }
    }

    /// The body's bytes in the zone: its code, its data words and the
    /// padding after them.
    fn size(&self) -> u32 {
        self.end - self.start
    }
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
    /// Every call's return address in the zone, as an offset from its
    /// base, with what the leaves a callee calls read of the send:
    /// looked up by the return address the callee finds on the native
    /// stack, as a binary search over `bodies` and then over the body's
    /// returns, which the leaf stub's leaf made on every call, was not.
    returns: IdentityMap<u32, ReturnSite>,
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
    /// Whether `MakeFuture` lowers to an exit in the bodies compiled from
    /// now on (`set_futures_exit`).
    futures_exit: bool,
    /// The entry that compiles a body: its first under the JIT stress mode.
    compile_at_entry: u32,
    /// The exit's walk over the compiled frames' return addresses, while
    /// the exit stub makes it (`materialize`).
    #[cfg(ns_jit)]
    pub(crate) walk: materialize::Walk,
    /// The cut's walk over the compiled frames' return addresses, while
    /// the cut stub makes it (`cut`).
    #[cfg(ns_jit)]
    pub(crate) cut: cut::CutWalk,
    /// The innermost frame the last walk materialized, for the run loop to
    /// go on with (`take_walked`).
    pub(crate) walked: Option<ActivationReference>,
    /// The block every entry hands machine code (`stubs::BlockWord`), kept
    /// from one entry to the next: an entry writes only the words that
    /// name its frame and its code, the others when `block_stale` says
    /// what they name has changed.
    block: EntryBlock,
    /// Whether a word of `block` that an entry does not write may no
    /// longer name what it named when it was written: the zone made or
    /// replaced, the global cache resized (`mark_block_stale`).
    block_stale: bool,
    /// Whether a compile found no room in the zone since its last
    /// compaction: the run loop's next safepoint compacts it
    /// (`Mutator::compact_code_zone`), as Cog only sets a flag when a
    /// compile finds no room (`callForCogCompiledCodeCompaction`,
    /// `cointerp.c:12068-12074`) and compacts at its next event check.
    compaction_requested: bool,
    /// Whether a compile found no room and no compaction has freed a
    /// quarter of the zone since, nor a full collection run: a body that
    /// found no room goes on counting but compiles nothing until the room
    /// left holds it (`may_compile_now`), and a compile that finds no room
    /// asks for no compaction, so that a zone full of active code neither
    /// assembles a hot body again at every second entry nor compacts at
    /// every safepoint. A compaction is asked for again once enough
    /// compiles were held (`held_compiles`), so that the exhaustion ends
    /// without a collection.
    zone_exhausted: bool,
    /// The most bytes a compile that found no room needed since the zone
    /// last had room -- a compaction that freed that much, or a full
    /// collection -- its alignment's slack included: a compaction frees
    /// at least as much, beside its quarter of the zone, so that a zone of
    /// a few bodies' room makes room for the body that asked, and a retry
    /// of an exhausted zone ends the exhaustion only once it has.
    wanted_bytes: u32,
    /// The compiles held since the last compaction because the zone was
    /// exhausted: entries of bodies that came due, each counted once per
    /// entry. At `zone::compiles_held_before_retry` of them a compaction
    /// is asked for again.
    held_compiles: u32,
    /// The compactions in a row that left the zone exhausted, which
    /// double the compiles held before the next is asked for; a compaction
    /// that ends the exhaustion resets it.
    fruitless_compactions: u32,
    /// The bytes of the bodies freed since the last compaction: Cog's
    /// `methodBytesFreedSinceLastCompaction` (`cogitX64SysV.c:12887`). A
    /// full collection that finds a quarter of the bodies' room so freed
    /// asks for a compaction (`JitState::note_full_collection`).
    bytes_freed_since_compaction: u32,
    /// How many entries into machine code are in progress, raised around
    /// `CodeZone::enter`: a compaction, which moves the code, asserts there
    /// are none. Rust's alone; machine code never reads it. Debug builds
    /// only: only their assertions read it.
    #[cfg(debug_assertions)]
    entries_in_progress: u32,
    /// The bodies abandoned and not yet revived, held weakly as the
    /// installed bodies are, so that a body dies with its mixin's row:
    /// what `decay_exits` halves the exits of beside the installed bodies,
    /// whose slots no longer name machine code.
    abandoned: Vec<Weak<CompiledCode>>,
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

/// Whether `NS_NEXT_STRESS_ZONE` asks for the zone stress mode: set, and
/// neither empty nor `0`. Debug builds only; read once per process. The
/// interpreter's safepoint then compacts the code zone at the safepoints
/// its schedule picks, freeing every body no activation runs
/// (`Interpreter::safepoint`); like the exit stress mode, it forces the
/// JIT on where it is available.
pub(crate) fn stress_zone_from_env() -> bool {
    static REQUESTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    cfg!(debug_assertions)
        && *REQUESTED.get_or_init(|| {
            std::env::var_os("NS_NEXT_STRESS_ZONE")
                .is_some_and(|value| !value.is_empty() && value != "0")
        })
}

impl JitState {
    /// The state of a new interpreter: a zone of `DEFAULT_ZONE_BYTES`,
    /// unless `NS_NEXT_NO_JIT=1` turns the JIT off.
    pub(crate) fn new() -> JitState {
        let stress_jit = stress_jit_from_env();
        let stress = stress_jit || stress_exit_from_env() || stress_zone_from_env();
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
            returns: IdentityMap::default(),
            counting: false,
            statistics: JitStatistics::default(),
            install_bodies_exiting_at_once: stress_jit,
            futures_exit: false,
            compile_at_entry: if stress_jit { 1 } else { COMPILE_AT_ENTRY },
            #[cfg(ns_jit)]
            walk: materialize::Walk::default(),
            #[cfg(ns_jit)]
            cut: cut::CutWalk::default(),
            walked: None,
            block: EntryBlock::new(),
            block_stale: true,
            compaction_requested: false,
            zone_exhausted: false,
            wanted_bytes: 0,
            held_compiles: 0,
            fruitless_compactions: 0,
            bytes_freed_since_compaction: 0,
            #[cfg(debug_assertions)]
            entries_in_progress: 0,
            abandoned: Vec::new(),
        }
    }

    /// Makes the next entry write every word of the entry block afresh:
    /// called wherever what one of them names changes -- the zone made or
    /// replaced, the global cache resized. A new lasting word of the block
    /// (`lasting_block_words`), or a new way to replace what one names,
    /// must call it too: only a debug build's check at every entry
    /// catches a miss, which a release build runs on with the stale word.
    pub(crate) fn mark_block_stale(&mut self) {
        self.block_stale = true;
    }

    pub(crate) fn statistics(&self) -> JitStatistics {
        self.statistics
    }

    /// The bytes of the zone installed code takes, the bodies installed
    /// and the call returns recorded (`Interpreter::table_sizes`); zeros
    /// without a zone.
    pub(crate) fn zone_sizes(&self) -> (usize, usize, usize) {
        let used_bytes = self
            .zone
            .as_ref()
            .map_or(0, |zone| zone.next_offset as usize);
        (used_bytes, self.bodies.len(), self.returns.len())
    }

    /// Whether bodies are compiled and entered: the one load an
    /// interpreter's send of a block makes before it looks for the
    /// block's machine code (`Mutator::enter_sent`).
    #[inline(always)]
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
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
            self.mark_block_stale();
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
        self.returns.clear();
        self.compaction_requested = false;
        self.zone_exhausted = false;
        self.wanted_bytes = 0;
        self.held_compiles = 0;
        self.fruitless_compactions = 0;
        self.bytes_freed_since_compaction = 0;
        self.zone = None;
        if available() {
            self.zone = reserve_zone(bytes);
        }
        self.enabled = self.enabled && self.zone.is_some();
        self.mark_block_stale();
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

    /// Makes `MakeFuture` lower to an exit in the bodies compiled from now
    /// on (`true`), as no body's does otherwise, so that a body making a
    /// future at once exits at once: what a test of a body never compiled
    /// needs, since every other instruction such a body could start with
    /// runs in machine code (`Interpreter::set_jit_futures_exit`).
    pub(crate) fn set_futures_exit(&mut self, futures_exit: bool) {
        self.futures_exit = futures_exit;
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
    #[cfg(all(test, debug_assertions))]
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
        let site = self.call_return(return_address)?;
        let body = self.live_body(site.body)?;
        Some((upgraded(body)?, site.program_counter))
    }

    /// The send whose call returns to `return_address`, as `returns`
    /// records it.
    pub(crate) fn call_site(&self, return_address: u64) -> Option<CallSiteOperands> {
        let site = self.call_return(usize::try_from(return_address).ok()?)?;
        Some(CallSiteOperands {
            program_counter: site.program_counter,
            argument_base: site.argument_base,
            argument_count: site.argument_count,
            selector: site.selector,
        })
    }

    /// What `returns` records of the call that returns to
    /// `return_address`.
    fn call_return(&self, return_address: usize) -> Option<ReturnSite> {
        let zone = self.zone.as_ref()?;
        let base = zone.writer.zone().executable_address(CodeOffset(0));
        let offset = u32::try_from(return_address.checked_sub(base)?).ok()?;
        self.returns.get(&offset).copied()
    }

    /// The body an address inside the code lies in, by the byte before it
    /// as `body_at` looks it up -- an address `MacroAssembler::exit` or a
    /// leaf call's `Operand::CodeAddress` handed over -- its method and its
    /// feedback base.
    pub(crate) fn body_of(&self, address: usize) -> Option<Installed> {
        // A call's return address, which every leaf a callee calls hands
        // over, is found without a search.
        if let Some(site) = self.call_return(address) {
            return upgraded(self.live_body(site.body)?);
        }
        upgraded(self.installed_body_at(address)?)
    }

    /// The sending frame's register 0 and the program counter of the send
    /// whose call returns to `return_address` and whose callee's register
    /// 0 lies at `callee_address`: the callee lies on the send's argument
    /// slots.
    pub(crate) fn sender_of(&self, callee_address: u64, return_address: u64) -> Option<(u64, u32)> {
        let site = self.call_return(usize::try_from(return_address).ok()?)?;
        let sender = callee_address.checked_sub(u64::from(site.argument_base) * 8)?;
        Some((sender, site.program_counter))
    }

    /// Writes `words` at `address`, data words of the installed body the
    /// address inside the code `code_address` lies in, through the zone's
    /// one writer (`MacroAssembler::take_body_words`): machine code reads
    /// them through the executable view, and the writer is the one way
    /// anything is stored into the zone.
    #[cfg(ns_jit)]
    pub(crate) fn write_body_words(&mut self, code_address: usize, address: usize, words: &[u64]) {
        let Some(body) = self.installed_body_at(code_address) else {
            unreachable!("internal: body words are written for an installed body");
        };
        let (start, end) = (body.words, body.words_end);
        let Some(zone) = self.zone.as_mut() else {
            unreachable!("internal: an installed body lies in a zone");
        };
        let base = zone.writer.zone().executable_address(CodeOffset(0));
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        let offset = address
            .checked_sub(base)
            .and_then(|offset| u32::try_from(offset).ok())
            .filter(|offset| {
                // Only data words, never an instruction, are written.
                *offset >= start
                    && u32::try_from(bytes.len())
                        .ok()
                        .and_then(|length| offset.checked_add(length))
                        .is_some_and(|written_end| written_end <= end)
            });
        let Some(offset) = offset else {
            unreachable!("internal: a body's words lie inside the body");
        };
        if zone.writer.write(CodeOffset(offset), &bytes).is_err() {
            unreachable!("internal: an installed body's words lie in the committed zone");
        }
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
        (offset < body.end && !body.free).then_some(body)
    }

    /// The installed body at `index` in `bodies`, unless it is freed.
    fn live_body(&self, index: u32) -> Option<&InstalledBody> {
        self.bodies.get(index as usize).filter(|body| !body.free)
    }

    /// A state with the JIT on and a zone of `bytes`, whatever the
    /// environment says, installing and entering at a push every body it
    /// compiles when `install_bodies_exiting_at_once` is set.
    #[cfg(all(test, ns_jit))]
    fn for_test(bytes: usize, install_bodies_exiting_at_once: bool) -> JitState {
        let zone = reserve_zone(bytes).expect("a zone");
        JitState {
            zone: Some(zone),
            bodies: Vec::new(),
            returns: IdentityMap::default(),
            enabled: true,
            counting: false,
            statistics: JitStatistics::default(),
            install_bodies_exiting_at_once,
            futures_exit: false,
            compile_at_entry: COMPILE_AT_ENTRY,
            #[cfg(ns_jit)]
            walk: materialize::Walk::default(),
            #[cfg(ns_jit)]
            cut: cut::CutWalk::default(),
            walked: None,
            block: EntryBlock::new(),
            block_stale: true,
            compaction_requested: false,
            zone_exhausted: false,
            wanted_bytes: 0,
            held_compiles: 0,
            fruitless_compactions: 0,
            bytes_freed_since_compaction: 0,
            #[cfg(debug_assertions)]
            entries_in_progress: 0,
            abandoned: Vec::new(),
        }
    }
}

/// An installed body, while it lives: its code, its method when it is
/// one's, and the feedback base it was compiled against.
#[derive(Clone)]
pub(crate) struct Installed {
    pub(crate) code: Rc<CompiledCode>,
    pub(crate) method: Option<Rc<MixinMethod>>,
    pub(crate) feedback_base: FeedbackBase,
    /// Whether the body reifies its frame (`InstalledBody::reifies`).
    pub(crate) reifies: bool,
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
        reifies: body.reifies,
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
        bodies_start: 0,
        entry_stub: CodeOffset(0),
        call_stub: CodeOffset(0),
        closure_call_stub: CodeOffset(0),
        exit_stub: CodeOffset(0),
        miss_stub: CodeOffset(0),
        refusing_stub: CodeOffset(0),
        cut_stub: CodeOffset(0),
        polymorphic_stub: CodeOffset(0),
        megamorphic_stub: CodeOffset(0),
        accessors: AccessorStubs {
            getters: Vec::new(),
            setters: Vec::new(),
            parameters: Vec::new(),
            mirror_methods: Vec::new(),
            row_stubs: Vec::new(),
            nil: CodeOffset(0),
            true_constant: CodeOffset(0),
            false_constant: CodeOffset(0),
            receiver: CodeOffset(0),
            act: CodeOffset(0),
            guarded_getters: Vec::new(),
            guarded_setters: Vec::new(),
            nested_classes: Vec::new(),
            closure_values: Vec::new(),
            values_of_parameters: Vec::new(),
            culls: Vec::new(),
        },
    };
    zone.entry_stub = install(&mut zone, &stubs::assemble_entry_stub(Abi::host()).bytes).ok()?;
    zone.call_stub = install(&mut zone, &stubs::assemble_call_stub(Abi::host()).bytes).ok()?;
    zone.closure_call_stub = install(
        &mut zone,
        &stubs::assemble_closure_call_stub(Abi::host()).bytes,
    )
    .ok()?;
    zone.exit_stub = install(&mut zone, &stubs::assemble_exit_stub(Abi::host()).bytes).ok()?;
    zone.miss_stub = install(&mut zone, &stubs::assemble_miss_stub(Abi::host()).bytes).ok()?;
    zone.refusing_stub =
        install(&mut zone, &stubs::assemble_refusing_stub(Abi::host()).bytes).ok()?;
    zone.cut_stub = install(&mut zone, &stubs::assemble_cut_stub(Abi::host()).bytes).ok()?;
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
        setters: entries
            .setters
            .iter()
            .map(|offset| placed(*offset))
            .collect(),
        parameters: entries
            .parameters
            .iter()
            .map(|offset| placed(*offset))
            .collect(),
        mirror_methods: entries
            .mirror_methods
            .iter()
            .map(|offset| placed(*offset))
            .collect(),
        row_stubs: entries
            .row_stubs
            .iter()
            .map(|offset| placed(*offset))
            .collect(),
        nil: placed(entries.nil),
        true_constant: placed(entries.true_constant),
        false_constant: placed(entries.false_constant),
        receiver: placed(entries.receiver),
        act: placed(entries.act),
        guarded_getters: entries
            .guarded_getters
            .iter()
            .map(|offset| placed(*offset))
            .collect(),
        guarded_setters: entries
            .guarded_setters
            .iter()
            .map(|offset| placed(*offset))
            .collect(),
        nested_classes: entries
            .nested_classes
            .iter()
            .map(|offset| placed(*offset))
            .collect(),
        closure_values: entries
            .closure_values
            .iter()
            .map(|offset| placed(*offset))
            .collect(),
        values_of_parameters: entries
            .values_of_parameters
            .iter()
            .map(|offset| placed(*offset))
            .collect(),
        culls: entries.culls.iter().map(|offset| placed(*offset)).collect(),
    };
    zone.bodies_start = zone.next_offset;
    Some(zone)
}

#[cfg(not(ns_jit))]
fn reserve_zone(_bytes: usize) -> Option<Zone> {
    None
}

/// Writes `bytes` at the zone's next aligned offset and answers it. Machine
/// code may be running from the zone meanwhile -- a leaf installs a callee
/// -- which holds no address an install moves: an install only appends,
/// and only a compaction, between entries, moves code (`jit::zone`).
fn install(zone: &mut Zone, bytes: &[u8]) -> Result<CodeOffset, Refusal> {
    install_aligned(zone, bytes, 1)
}

/// `install`, the code placed at a multiple of `alignment` bytes as well,
/// so that the data its assembler aligned from its start
/// (`Assembled::alignment`) is aligned in the zone.
fn install_aligned(zone: &mut Zone, bytes: &[u8], alignment: u32) -> Result<CodeOffset, Refusal> {
    let offset = zone
        .next_offset
        .checked_next_multiple_of(alignment.max(BODY_ALIGNMENT))
        .ok_or(Refusal::ZoneFull)?;
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
/// rewrites the site's cell (`inline_cache::decide_at`). A factory's
/// body is reached the same way.
///
/// A target that is no method with code is reached through the stub
/// `stub_entry` chooses for it.
///
/// A body whose machine entry the cell is to name has its usage raised
/// (`InstalledBody::note_linked`): a cell is written only from its entry,
/// on the cold path of a miss, a fill or a refresh, so the hot path pays
/// nothing for the count, as Cog raises it only when it links a send.
pub(crate) fn cell_entry(
    state: &JitState,
    target: &crate::interpreter::inline_cache::Target,
) -> u64 {
    entry_of_target(state, target, true)
}

/// `cell_entry` as a check of a cell reads it, raising no body's usage:
/// the verification of the cells (`InlineCaches::verify_cells`) must not
/// change which bodies a compaction frees.
pub(crate) fn expected_cell_entry(
    state: &JitState,
    target: &crate::interpreter::inline_cache::Target,
) -> u64 {
    entry_of_target(state, target, false)
}

/// `cell_entry`, raising the usage of the body it answers the machine
/// entry of when `noting`.
fn entry_of_target(
    state: &JitState,
    target: &crate::interpreter::inline_cache::Target,
    noting: bool,
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
            let body = state
                .installed(&method.code)
                .filter(|body| body.feedback_base == *feedback_base);
            return match body {
                Some(body) => {
                    if noting {
                        body.note_linked();
                    }
                    address(CodeOffset(body.start))
                }
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
        // A factory's body, entered on the class object the send
        // instantiates, as its receiver and its current class
        // (`baseline::BodyRole::Factory`). It has no sends, so its machine
        // code is compiled against the one base a body without sends has.
        // The miss refuses a factory whose body takes another count than
        // the send places (`inline_cache::decide_at`); a hit cannot meet
        // one: a body takes fewer than its factory's arguments only when
        // it stands for more than `u16::MAX` of them
        // (`CompiledCode::stands_for_too_many_arguments`), and no send
        // with a cell places that many, since a body numbers its
        // registers, and so its sends' slots, in 16 bits.
        Target::Factory { code, .. } => {
            return match state.installed(code) {
                Some(body) => {
                    if noting {
                        body.note_linked();
                    }
                    address(CodeOffset(body.start))
                }
                None if state.may_compile(code) => address(zone.miss_stub),
                None => address(zone.refusing_stub),
            };
        }
        Target::NestedClass { field, chain, .. } => Stubbed::NestedClass {
            chain: *chain,
            field: *field,
        },
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
    /// A nested class's accessor, its class cached in hidden slot `field`
    /// (`nested::NO_CACHED_FIELD` where no slot was recorded).
    NestedClass {
        chain: crate::interpreter::resolve::Chain,
        field: u32,
    },
}

/// The stub that does what `Mutator::act_with_base` does for `stubbed`
/// (`targets::assemble_accessor_stubs`; `docs/design/execution.md`,
/// "Sends in machine code"), chosen from it alone: a getter on a public
/// or protected chain, whose guard is the receiver's own, through its
/// field's getter stub; a quick method's constant, receiver or parameter
/// through its stub, read off the method, which is immutable, as `act`
/// reads it on every hit; a setter on a public or protected chain through
/// its field's setter stub; a method whose fast path is a leaf-safe mirror
/// row through its row's own stub (`MachineCall::Stub`) or, without one
/// (`MachineCall::Leaf`), its row's mirror method stub; a method whose
/// fast path activates a block -- a row that activates its receiver
/// (`BlockPrimitive::Value`), `^<parameter> value` and `^<parameter>
/// cull: self` of at most `SHIFTED_SLOTS` arguments -- through
/// its closure stub (`targets::ClosureStub`); a private or
/// `super` slot accessor through its field's guarded stub, which makes
/// the guard check's first compare inline and goes on to the leaf stub
/// for the rest; a getter or setter past the stubs' fields, a builtin
/// class's accessor and a primitive that is a mirror send of a leaf-safe
/// row through the leaf stub. `None` for every other -- a row that is not
/// leaf-safe, a quick method activating a block at a send of more
/// arguments than a closure stub shifts past, a `doesNotUnderstand:`
/// handler -- and where there is no zone: the interpreter runs the send.
pub(crate) fn stub_entry(state: &JitState, stubbed: &Stubbed<'_>) -> Option<u64> {
    use crate::builtins::primitive::{mirror_entry, BlockPrimitive, MachineCall, Primitive};
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
    let leaf_safe = |row| mirror_entry(row).machine_call.is_leaf_safe();
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
            // A row that activates its receiver, a closure, with the send's
            // arguments.
            Some(MethodFastPath::Mirror(row))
                if mirror_entry(row).primitive == Primitive::Block(BlockPrimitive::Value) =>
            {
                u8::try_from(method.argument_count)
                    .ok()
                    .and_then(|arity| zone.accessors.closure_values.get(usize::from(arity)))
                    .map(|entry| address(*entry))
            }
            // A position past the send's arguments activates the method,
            // as `Parameter`'s does.
            Some(MethodFastPath::ValueOfParameter(position)) => {
                parameter_stub_index(method.argument_count, usize::from(position))
                    .and_then(|index| zone.accessors.values_of_parameters.get(index))
                    .map(|entry| address(*entry))
            }
            Some(MethodFastPath::CullParameter(position)) => {
                parameter_stub_index(method.argument_count, usize::from(position))
                    .and_then(|index| zone.accessors.culls.get(index))
                    .map(|entry| address(*entry))
            }
            Some(MethodFastPath::Mirror(row)) => match mirror_entry(row).machine_call {
                MachineCall::Stub(stub) => Some(address(zone.accessors.row_stubs[stub.index()])),
                MachineCall::Leaf => Some(
                    zone.accessors
                        .mirror_methods
                        .get(row.index())
                        .map_or(act, |entry| address(*entry)),
                ),
                MachineCall::Exit => None,
            },
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
        Stubbed::Slot {
            kind: AccessorKind::Setter,
            chain: Chain::Public | Chain::Protected,
            field,
        } => Some(
            zone.accessors
                .setters
                .get(*field)
                .map_or(act, |entry| address(*entry)),
        ),
        Stubbed::Slot {
            kind,
            chain: Chain::Private | Chain::Super,
            field,
        } => Some(
            match kind {
                AccessorKind::Getter => &zone.accessors.guarded_getters,
                AccessorKind::Setter => &zone.accessors.guarded_setters,
            }
            .get(*field)
            .map_or(act, |entry| address(*entry)),
        ),
        Stubbed::Slot { .. } => Some(act),
        Stubbed::Primitive {
            primitive: Primitive::Mirror(row),
            ..
        } if leaf_safe(*row) => Some(act),
        Stubbed::Primitive { .. } => None,
        // Only on a chain whose cell compares the receiver's own guard is
        // the recorded slot the receiver's; any other is the interpreter's.
        Stubbed::NestedClass {
            chain: Chain::Public | Chain::Protected,
            field,
        } => zone
            .accessors
            .nested_classes
            .get(usize::try_from(*field).unwrap_or(usize::MAX))
            .map(|entry| address(*entry)),
        Stubbed::NestedClass { .. } => None,
    }
}

/// The most arguments a send whose quick method activates a block may
/// place: a closure stub shifts the block's frame up past all the send's
/// slots, which its flags word holds in two bits
/// (`frames::CALL_SHIFT_BITS`), and so does the stub of a block row, whose
/// arities `targets::CLOSURE_VALUE_ARITIES` keeps within the same bound.
/// Every such quick method of the core library takes at most two.
pub(crate) const SHIFTED_SLOTS: u8 = 3;

/// The index of the closure stub of `^<parameter> value` or
/// `^<parameter> cull: self` for a send of `slots` arguments whose block
/// is the one at `position` (`targets::ClosureStub`): the entries of one
/// slot count, then of the next.
pub(crate) fn parameter_stub_index(slots: usize, position: usize) -> Option<usize> {
    (slots >= 1 && slots <= usize::from(SHIFTED_SLOTS) && position < slots)
        .then(|| slots * (slots - 1) / 2 + position)
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

    /// The address of the stub a `NonLocalReturn`'s template calls to cut
    /// the return (`stubs::assemble_cut_stub`); 0 where there is no zone.
    pub(crate) fn cut_stub_address(&self) -> u64 {
        self.zone.as_ref().map_or(0, |zone| {
            zone.writer.zone().executable_address(zone.cut_stub) as u64
        })
    }

    /// The address of the stub answering its receiver, which a compiled
    /// initializer invocation calls for a decision that runs no code
    /// (`jit::leaves`'s `initializer`); 0 where there is no zone.
    pub(crate) fn receiver_stub_address(&self) -> u64 {
        self.zone.as_ref().map_or(0, |zone| {
            zone.writer
                .zone()
                .executable_address(zone.accessors.receiver) as u64
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
        compiled.then(|| self.live_body(slot.map.get())).flatten()
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
    /// Program counter 0, a push's, is read off the slot rather than
    /// searched for in the map.
    fn interpreter_entry(
        &self,
        code: &CompiledCode,
        feedback_base: FeedbackBase,
        program_counter: u32,
    ) -> Option<CodeOffset> {
        let body = self.installed(code)?;
        if body.feedback_base != feedback_base {
            return None;
        }
        if program_counter == 0 {
            return Some(CodeOffset(code.jit.first_instruction.get()));
        }
        let offset = body.map.machine_offset(program_counter)?;
        Some(CodeOffset(body.start + offset))
    }

    /// Whether an entry into `code` has nothing to count: the JIT is on
    /// and the body is compiled into this zone. Every
    /// other body's entry is `note_entry`'s -- one still counting, one
    /// compiled into another zone, and any while the JIT is off, which
    /// `note_entry` refuses.
    fn is_entered_here(&self, code: &CompiledCode) -> bool {
        self.enabled
            && code.jit.state.get() == COMPILED
            && code.jit.zone.get() == self.zone_identity()
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

    /// Halves the exit count of every body compiled into this zone and of
    /// every abandoned one, and revives each abandoned body whose count
    /// fell below `REVIVAL_EXITS`: it counts its entries again
    /// (`JitSlot::forget`), its machine code long freed (`abandon`).
    /// Answers whether one was revived, in which case the caller writes
    /// every cell afresh from its entry (`InlineCaches::refresh_all_cells`),
    /// so that a cell that refused the call of a revived body names the
    /// miss stub again, which compiles it, and its callers stop spending
    /// their budgets on it.
    ///
    /// Run after every scavenge and full collection (`World::scavenge`,
    /// `World::collect`): the run's periodic event, as Strongtalk's zone
    /// sweeper divides each nmethod's uncommon-trap counter by its decay
    /// factor at every step (`nmethod::sweeper_step`,
    /// `prior-art/strongtalk/vm/code/nmethod.cpp:960-966`, run for each
    /// nmethod by `ZoneSweeper::nmethod_task`, `vm/runtime/sweeper.cpp:218`).
    /// A body that always exits is so compiled at most once per two of
    /// them. The JIT stress mode never abandons a body
    /// (`resumption_after_exit`), so it never revives one either.
    pub(crate) fn decay_exits(&mut self) -> bool {
        for (index, body) in self.bodies.iter().enumerate() {
            // Only the body its slot names, so that each slot's count is
            // halved once: one freed, or compiled anew since, is not.
            if let Some(code) = body.code.upgrade() {
                if self.named_body(&code) == Some(index) {
                    code.jit.decay_exits();
                }
            }
        }
        let mut revived = 0;
        self.abandoned.retain(|body| {
            let Some(code) = body.upgrade() else {
                return false;
            };
            // A body refused since -- entered while the JIT was off -- is
            // no longer abandoned.
            if code.jit.state.get() != ABANDONED {
                return false;
            }
            if code.jit.decay_exits() >= REVIVAL_EXITS {
                return true;
            }
            code.jit.forget();
            revived += 1;
            false
        });
        self.statistics.revived_bodies += revived;
        revived > 0
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
/// A body that cannot be compiled is refused for good; one that found no
/// room in the zone counts again, and while the zone stays exhausted
/// (`JitState::zone_exhausted`) entries of a body known not to fit the
/// room left are counted but compile nothing, each held compile counting
/// towards the next compaction (`JitState::hold_compile`).
/// A body compiled into a zone that is not this interpreter's -- another
/// interpreter's, or one since replaced -- is compiled again into this
/// one.
///
/// A caller that holds the mutator passes the compaction a full zone asks
/// for on to the heap's safepoint request (`request_compaction`).
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
        REFUSED | ABANDONED | EXITS_AT_ONCE => {}
        COMPILED if slot.zone.get() == state.zone_identity() => {}
        // A body two interpreters ran in turn would be compiled again at
        // every switch, each time leaving its old code in the other's
        // zone; no body is run by two interpreters today
        // (`docs/OPEN-WORK.md`, road step 8).
        COMPILED => {
            if state.may_compile_now(slot) {
                compile_into_slot(state, code, feedback_base, method);
            } else {
                state.hold_compile();
            }
        }
        entries => {
            let entries = entries + 1;
            if entries < state.compile_at_entry {
                slot.state.set(entries);
            } else if state.may_compile_now(slot) {
                compile_into_slot(state, code, feedback_base, method);
            } else {
                // Held at the entry that compiles, so that the first entry
                // after the zone has room again compiles a body that came
                // due meanwhile.
                slot.state.set(state.compile_at_entry);
                state.hold_compile();
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
        && state.may_compile_now(&code.jit)
        && match code.jit.state.get() {
            REFUSED | ABANDONED | EXITS_AT_ONCE => false,
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
        Ok((offsets, map)) => {
            code.jit.compiled(state.zone_identity(), offsets, map);
            state.statistics.compiled_bodies += 1;
        }
        Err(Refusal::ExitsAtOnce) => {
            code.jit.settle(EXITS_AT_ONCE);
            state.statistics.bodies_exiting_at_once += 1;
        }
        // No room is no reason to refuse the body for good: it counts its
        // entries again, and the zone is compacted at the next safepoint,
        // as Cog forgets which methods it could not compile at every
        // compaction (`nilUncoggableMethods`, `cointerp.c:14432-14433`).
        // A compile from inside a leaf, which cannot compact, only sets
        // the flags.
        // While the zone is already exhausted, a body that does not fit
        // the room left asks for nothing: only the held compiles do, so
        // that the retries keep their backoff.
        Err(Refusal::ZoneFull) => {
            code.jit.forget();
            if state.zone_exhausted {
                state.hold_compile();
            } else {
                state.compaction_requested = true;
                state.zone_exhausted = true;
            }
        }
        Err(Refusal::TooManyInstructions | Refusal::Unassembled | Refusal::TooLarge) => {
            code.jit.refuse();
            state.statistics.refused_bodies += 1;
        }
    }
}

/// Passes a compaction a compile asked for (`JitState::compaction_requested`)
/// on to the heap, whose safepoint request the run loop polls: called
/// after every compile trigger that holds the mutator -- a push, a miss, an
/// initializer invocation, a future forced -- since `note_entry` itself
/// reaches only the JIT's state. Only a request: a leaf may call it, and
/// the compaction waits for the run loop's safepoint, where no return
/// address into the zone exists.
pub(crate) fn request_compaction(mutator: &mut Mutator<'_>) {
    if mutator.jit.compaction_requested {
        mutator.heap.request_safepoint();
    }
}

/// Compiles `code`, its sends' cells those of `feedback_base`'s range,
/// and installs it, answering its offset and that of its instruction at
/// program counter 0 and its index among the installed bodies.
#[cfg(ns_jit)]
fn compile(
    state: &mut JitState,
    code: &Rc<CompiledCode>,
    feedback_base: FeedbackBase,
    method: Option<&Rc<MixinMethod>>,
) -> Result<([CodeOffset; 2], u32), Refusal> {
    let Some(zone) = state.zone.as_mut() else {
        panic!("internal: the JIT is on without a zone");
    };
    let mut macro_assembler = macro_assembler::MacroAssembler::new(macro_assembler::Abi::host());
    macro_assembler.set_count_back_edges(state.counting);
    let role = baseline::BodyRole::of(code, method.is_some());
    let body = baseline::compile(
        code,
        macro_assembler,
        feedback_base,
        role,
        state.futures_exit,
    )?;
    if body.exits_at_once && !state.install_bodies_exiting_at_once {
        return Err(Refusal::ExitsAtOnce);
    }
    let map = u32::try_from(state.bodies.len()).map_err(|_| Refusal::ZoneFull)?;
    // Every body has an instruction, its last a return or a jump, so a
    // map without program counter 0 is the JIT's own defect.
    let Some(first_instruction) = body.map.machine_offset(0) else {
        panic!("internal: a compiled body has no code for its first instruction");
    };
    // A body the zone could not hold even with every other body freed is
    // refused for good, rather than compacting the zone in vain at every
    // second entry.
    let end_in_an_empty_zone = zone
        .bodies_start
        .checked_next_multiple_of(body.code.alignment.max(BODY_ALIGNMENT))
        .zip(u32::try_from(body.code.bytes.len()).ok())
        .and_then(|(start, length)| start.checked_add(length));
    if end_in_an_empty_zone.is_none_or(|end| end > zone.limit) {
        return Err(Refusal::TooLarge);
    }
    // The body's data words, its closure sites among them, are installed
    // with it, zeroed, so that each is filled at its first run; a zone too
    // full for the code and its words refuses both, and the compaction
    // that follows frees room for them at least.
    let offset = match install_aligned(zone, &body.code.bytes, body.code.alignment) {
        Ok(offset) => offset,
        Err(refusal) => {
            let needed = u32::try_from(body.code.bytes.len())
                .unwrap_or(u32::MAX)
                .saturating_add(body.code.alignment.max(BODY_ALIGNMENT));
            state.wanted_bytes = state.wanted_bytes.max(needed);
            code.jit.needed.set(needed);
            return Err(refusal);
        }
    };
    let length = u32::try_from(body.code.bytes.len()).map_err(|_| Refusal::ZoneFull)?;
    for call in &body.map.returns {
        // Every call of a body is a cached send's or an initializer
        // invocation's (`sends`).
        let Some(slots) = sends::call_slots(code, call.program_counter) else {
            unreachable!("internal: a body calls only at a cached send or an initializer's");
        };
        state.returns.insert(
            offset.0 + call.machine_offset,
            ReturnSite {
                body: map,
                program_counter: call.program_counter,
                argument_base: slots.argument_base,
                argument_count: slots.argument_count,
                selector: slots.selector,
            },
        );
    }
    state.bodies.push(InstalledBody {
        code: Rc::downgrade(code),
        method: method.map(Rc::downgrade),
        feedback_base,
        reifies: body.reifies,
        start: offset.0,
        end: offset.0 + length,
        words: offset.0 + body.words.as_ref().map_or(length, |words| words.start),
        words_end: offset.0 + body.words.as_ref().map_or(length, |words| words.end),
        map: body.map,
        alignment: body.code.alignment.max(BODY_ALIGNMENT),
        usage: Cell::new(zone::INITIAL_USAGE),
        free: false,
    });
    let first_instruction = CodeOffset(offset.0 + first_instruction);
    Ok(([offset, first_instruction], map))
}

#[cfg(not(ns_jit))]
fn compile(
    _state: &mut JitState,
    _code: &Rc<CompiledCode>,
    _feedback_base: FeedbackBase,
    _method: Option<&Rc<MixinMethod>>,
) -> Result<([CodeOffset; 2], u32), Refusal> {
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
/// program counter 0: counts the entry, unless the body is compiled into
/// this zone already, and runs the body's machine code when it has some
/// in this interpreter's zone. Out of line, so that the run loop pays only
/// `JitSlot::is_armed` for the JIT on a push.
#[cold]
#[inline(never)]
pub(crate) fn entered(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    code: &Rc<CompiledCode>,
    register_base: RegisterBase,
) -> Resumption {
    let feedback_base = mutator.heap.activation_feedback_base(activation);
    if !mutator.jit.is_entered_here(code) {
        // A top-level activation, which only a host pushes, has no kind
        // its code says: compiled as a factory's, its frame would be
        // reified and materialized as one. It stays interpreted.
        if matches!(
            mutator.heap.activation_kind(activation),
            crate::types::ActivationKind::TopLevel
        ) {
            code.jit.refuse();
            mutator.jit.statistics.refused_bodies += 1;
            return Resumption::Interpreted {
                activation,
                program_counter: 0,
            };
        }
        let method = mutator.heap.activation_method(activation);
        note_entry(&mut mutator.jit, code, feedback_base, method.as_ref());
        request_compaction(mutator);
    }
    // Every push of a block keeps its row current, the compiling one and
    // each after it: a full collection may have minted its id again, and
    // the interpreter calls (`called_closure`) only a block whose row is
    // written.
    #[cfg(ns_jit)]
    if code.kind == crate::bytecode::CodeKind::Block {
        let id = mutator.heap.activation_code_id(activation);
        note_block_entry(mutator, id, code);
    }
    // A body compiled at another push stays a body no top-level
    // activation runs: only a host pushes one, of code of its own.
    debug_assert!(
        !matches!(
            mutator.heap.activation_kind(activation),
            crate::types::ActivationKind::TopLevel
        ),
        "internal: machine code is entered for a top-level activation"
    );
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
/// call -- a cached send, an initializer invocation, or a `ResolveFuture`
/// that called a future's `resolve` (`sends::call_slots`) -- when its body
/// has machine code in this zone, or clears the bit and goes on
/// interpreted.
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
/// lie there.
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
    use stubs::{BlockWord, ExitKind};

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
    let code = Rc::clone(zone.writer.zone());
    let entry_stub = zone.entry_stub;
    let block = prepared_block(
        mutator,
        Heap::register_base_words(register_base),
        code.executable_address(entry) as u64,
    );
    mutator.jit.statistics.machine_entries += 1;
    mutator.jit.walked = None;
    let kind = enter_block(mutator, &code, entry_stub, &block);
    // A return first: the frame the run loop entered returned, so no frame
    // was materialized, none goes on interpreted and nothing exited.
    if kind == ExitKind::Returned as u64 {
        return Resumption::Returned {
            value: block.get(BlockWord::ExitProgramCounter.index()),
        };
    }
    // The innermost frame the exit materialized, or the entry's when no
    // compiled frame lay above it.
    let innermost = mutator.jit.take_walked().unwrap_or(activation);
    resumption_after_exit(mutator, &block, kind, innermost)
}

/// What an interpreter's send answers once it called a compiled
/// callee's machine code (`called`).
pub(crate) enum Called {
    /// Nothing ran: the body has no machine code in this zone for the
    /// step's feedback base, the native stack has too little room, or the
    /// callee's prologue refused the call before it wrote anything -- at
    /// the depth limit's bound, past the stack's room, or with a safepoint
    /// requested. The interpreter pushes the callee as it pushes a body
    /// without machine code, and so raises the overflow, grows the stack
    /// or takes the safepoint.
    Refused,
    /// The callee returned the value whose word this is, for the send's
    /// destination.
    Returned { value: u64 },
    /// Machine code exited above the interpreted sender: the run loop goes
    /// on at the innermost frame the exit materialized, interpreted or at
    /// a safepoint; never `Resumption::Returned`.
    Stopped(Resumption),
}

/// An interpreter's send of `sender`, whose registers start at
/// `register_base`, decided to activate `code`'s method on `receiver`
/// with `defining_class` as its current class, the arguments in the
/// send's slots from register `argument_base`, at `feedback_base`: calls
/// the body's machine entry through the call stub, as a compiled send
/// calls it, when it has machine code in this zone compiled against
/// `feedback_base` and the native stack has room
/// (`docs/design/execution.md`, "The entry at a send"). The callee's
/// prologue checks and counts the push and writes the frame as a
/// compiled call's callee does, so no frame is pushed here, and none is
/// popped once it returns. Out of line, so that the run loop pays only
/// the test of the slot (`JitSlot::is_compiled`) on a send.
///
/// # Panics
///
/// On a debug build's read of a dead register, as `run_machine_code`.
#[cfg(ns_jit)]
#[cold]
#[inline(never)]
#[expect(
    clippy::too_many_arguments,
    reason = "what the interpreter's push takes, as the send decided it, apart"
)]
pub(crate) fn called<'gc>(
    mutator: &mut Mutator<'gc>,
    sender: ActivationReference,
    register_base: RegisterBase,
    argument_base: u16,
    receiver: Value<'gc>,
    defining_class: ClassReference<'gc>,
    code: &Rc<CompiledCode>,
    feedback_base: FeedbackBase,
) -> Called {
    let Some(machine_entry) = machine_entry_address(&mutator.jit, code, feedback_base) else {
        return Called::Refused;
    };
    if !native_stack_has_room() {
        return Called::Refused;
    }
    mutator.heap.hand_machine_callee(receiver, defining_class);
    call_through(
        mutator,
        |zone| zone.call_stub,
        sender,
        register_base,
        (argument_base, code.argument_count),
        machine_entry,
    )
}

/// An interpreter's send of `sender`, whose registers start at
/// `register_base`, decided to activate `closure`, whose body is `code`,
/// on its arguments in the send's slots from register `argument_base`, at
/// the closure's `feedback_base`, counted against the depth limit or
/// exempt as `counted` says, as `Mutator::push_step`'s `Step::Block` arm
/// decides it: calls the block's machine entry through the closure call
/// stub, as a closure stub calls it from machine code, when it has machine
/// code in this zone compiled against `feedback_base` and the native stack
/// has room (`docs/design/execution.md`, "Stage 4: closures, non-local
/// return, allocation and the barrier"). The block's prologue checks the
/// push and counts it as a method's does, so an exempt activation's count
/// is taken off first and given back once nothing stands in its place --
/// at its return and at a refusal -- while one an exit materialized stays
/// on the stack exempt, as its flags word says, and its pop takes nothing
/// off. Out of line, as `called` is.
///
/// # Panics
///
/// On a debug build's read of a dead register, as `run_machine_code`.
#[cfg(ns_jit)]
#[cold]
#[inline(never)]
#[expect(
    clippy::too_many_arguments,
    reason = "what the interpreter's push takes, as the send decided it, apart"
)]
pub(crate) fn called_closure<'gc>(
    mutator: &mut Mutator<'gc>,
    sender: ActivationReference,
    register_base: RegisterBase,
    argument_base: u16,
    closure: crate::heap::ObjectReference<'gc>,
    code: &Rc<CompiledCode>,
    feedback_base: FeedbackBase,
    counted: bool,
) -> Called {
    let Some(machine_entry) = machine_entry_address(&mutator.jit, code, feedback_base) else {
        return Called::Refused;
    };
    if !native_stack_has_room() {
        return Called::Refused;
    }
    let Some(captured) = mutator.heap.closure_captured_frame(closure) else {
        unreachable!("internal: a block step is decided only for a closure that captured a frame");
    };
    // No row to note: the send calls only a block whose row names its
    // machine code already (`Heap::closure_has_machine_entry`).
    let receiver = mutator.heap.frame_receiver(captured);
    let current_class = mutator.heap.frame_current_class(captured);
    mutator
        .heap
        .hand_machine_closure(receiver, current_class, closure, counted);
    if !counted {
        mutator.heap.adjust_counted_depth(-1);
    }
    let called = call_through(
        mutator,
        |zone| zone.closure_call_stub,
        sender,
        register_base,
        (argument_base, code.argument_count),
        machine_entry,
    );
    if !counted && !matches!(called, Called::Stopped(_)) {
        mutator.heap.adjust_counted_depth(1);
    }
    called
}

/// The call `called` and `called_closure` make, once the callee's machine
/// entry is known and what its prologue is handed lies in the machine
/// words: through the stub `stub` picks, its walk seeded with the sender,
/// whose send's callee lies on its slots from register `argument_base`,
/// as many as the callee takes (`slots`).
#[cfg(ns_jit)]
fn call_through(
    mutator: &mut Mutator<'_>,
    stub: impl FnOnce(&Zone) -> CodeOffset,
    sender: ActivationReference,
    register_base: RegisterBase,
    (argument_base, slots): (u16, u16),
    machine_entry: u64,
) -> Called {
    use stubs::{BlockWord, ExitKind};

    let Some(zone) = mutator.jit.zone.as_ref() else {
        unreachable!("internal: a body has machine code only in a zone");
    };
    let zone_code = Rc::clone(zone.writer.zone());
    let call_stub = stub(zone);
    let sender_registers = Heap::register_base_index(register_base);
    let top = mutator.heap.activation_stack_top();
    mutator.jit.walk.seed_call(
        Heap::activation_offset(sender),
        sender_registers,
        argument_base,
        slots,
        top,
    );
    let block = prepared_block(
        mutator,
        Heap::register_base_words(register_base) + u64::from(argument_base),
        machine_entry,
    );
    mutator.jit.statistics.called_entries += 1;
    mutator.jit.walked = None;
    let kind = enter_block(mutator, &zone_code, call_stub, &block);
    if kind == ExitKind::Returned as u64 {
        // No exit took the seed.
        mutator.jit.walk.clear();
        return Called::Returned {
            value: block.get(BlockWord::ExitProgramCounter.index()),
        };
    }
    let Some(innermost) = mutator.jit.take_walked() else {
        unreachable!("internal: an exit of a called body materializes its innermost frame");
    };
    // A refused call that left the sender the innermost was refused by the
    // callee's own prologue: the interpreter pushes the callee rather than
    // run the send again, which would only call it again.
    if kind == ExitKind::PushRefused as u64
        && Heap::activation_offset(innermost) == Heap::activation_offset(sender)
    {
        // An exit all the same, which `resumption_after_exit` would have
        // counted.
        mutator.jit.statistics.exits += 1;
        return Called::Refused;
    }
    Called::Stopped(resumption_after_exit(mutator, &block, kind, innermost))
}

#[cfg(not(ns_jit))]
#[expect(
    clippy::too_many_arguments,
    reason = "what the interpreter's push takes, as the send decided it, apart"
)]
pub(crate) fn called<'gc>(
    _mutator: &mut Mutator<'gc>,
    _sender: ActivationReference,
    _register_base: RegisterBase,
    _argument_base: u16,
    _receiver: Value<'gc>,
    _defining_class: ClassReference<'gc>,
    _code: &Rc<CompiledCode>,
    _feedback_base: FeedbackBase,
) -> Called {
    unreachable!("internal: no body has machine code without an encoder")
}

#[cfg(not(ns_jit))]
#[expect(
    clippy::too_many_arguments,
    reason = "what the interpreter's push takes, as the send decided it, apart"
)]
pub(crate) fn called_closure<'gc>(
    _mutator: &mut Mutator<'gc>,
    _sender: ActivationReference,
    _register_base: RegisterBase,
    _argument_base: u16,
    _closure: crate::heap::ObjectReference<'gc>,
    _code: &Rc<CompiledCode>,
    _feedback_base: FeedbackBase,
    _counted: bool,
) -> Called {
    unreachable!("internal: no body has machine code without an encoder")
}

/// Records where the block body `code`, whose id is `id`, enters machine
/// code in the machine entry table the closure stubs read
/// (`heap/machine_entries.rs`), when it has machine code in this zone:
/// asked wherever the interpreter pushes a block -- `entered`, which
/// compiles it -- so that a body compiled, or a code id minted again after
/// a full collection freed it, is found by the closure stubs, and by an
/// interpreter's `value` send (`Mutator::enter_sent`), from the next call
/// on. A body compiled against no feedback base is left out: a closure
/// that has none is given its own at its first activation, which only the
/// interpreter's push does.
///
/// A row once written names its body until something clears it -- the
/// free of its id, the body's abandonment, the zone's replacement, a
/// switch of the JIT on or off -- so a row the table records is current, and a push of a block whose row is
/// recorded costs the test of a bit (`Heap::machine_entry_recorded`): the
/// body's lookup and the row's comparison cost every push of a block in
/// a run of closures a percent and more. A debug build compares the row
/// all the same (`Heap::record_machine_entry`).
#[cfg(ns_jit)]
pub(crate) fn note_block_entry(mutator: &mut Mutator<'_>, id: CodeId, code: &CompiledCode) {
    let recorded = mutator.heap.machine_entry_recorded(id);
    if !cfg!(debug_assertions) && recorded {
        return;
    }
    let Some(body) = mutator.jit.installed(code) else {
        return;
    };
    if body.feedback_base == FeedbackBase::NOT_ASSIGNED {
        return;
    }
    // A row written to name the body raises its usage, as a cell does
    // (`cell_entry`); a debug build's comparison of a recorded row writes
    // nothing, and raises nothing, so that both builds free alike.
    if !recorded {
        body.note_linked();
    }
    let (start, feedback_base) = (body.start, body.feedback_base);
    let Some(zone) = mutator.jit.zone.as_ref() else {
        return;
    };
    let entry = zone.writer.zone().executable_address(CodeOffset(start)) as u64;
    mutator
        .heap
        .record_machine_entry(id, entry, feedback_base, code.argument_count);
}

/// The entry block for an entry into the frame whose registers start
/// `register_base_words` words into the activation stack, at
/// `machine_entry`. The words that name the frame and the code are
/// written at every entry; the others name what does not move between
/// entries -- reservations, the stubs, the global cache's mask -- and are
/// written afresh only once something marked the block stale
/// (`JitState::mark_block_stale`), which a debug build checks at every
/// entry.
#[cfg(ns_jit)]
fn prepared_block(
    mutator: &mut Mutator<'_>,
    register_base_words: u64,
    machine_entry: u64,
) -> EntryBlock {
    use stubs::BlockWord;

    let Some(zone) = mutator.jit.zone.as_ref() else {
        panic!("internal: machine code is entered without a zone");
    };
    let exit_stub = zone.writer.zone().executable_address(zone.exit_stub) as u64;
    // Held apart from the mutator, which `enter` lends to the code.
    let block = mutator.jit.block.clone();
    if mutator.jit.block_stale {
        for (word, value) in lasting_block_words(mutator, exit_stub) {
            block.set(word.index(), value);
        }
        mutator.jit.block_stale = false;
    } else {
        debug_assert!(
            lasting_block_words(mutator, exit_stub)
                .iter()
                .all(|(word, value)| block.get(word.index()) == *value),
            "internal: a word of the entry block changed and nothing marked it stale"
        );
    }
    block.set(BlockWord::RegisterBase.index(), register_base_words);
    block.set(BlockWord::MachineEntry.index(), machine_entry);
    if mutator.jit.counting {
        block.set(BlockWord::BackEdges.index(), 0);
        block.set(BlockWord::CompiledCalls.index(), 0);
    }
    block
}

/// Enters `zone`'s machine code at `stub`, the entry or the call stub,
/// with `block`, and answers the kind of exit or return it left with, the
/// counters the code kept added to the statistics.
#[cfg(ns_jit)]
fn enter_block(
    mutator: &mut Mutator<'_>,
    zone: &ns_memory::CodeZone,
    stub: CodeOffset,
    block: &EntryBlock,
) -> u64 {
    use stubs::BlockWord;

    #[cfg(debug_assertions)]
    {
        mutator.jit.entries_in_progress += 1;
    }
    let kind = zone.enter(stub, block, mutator, leaves::table());
    #[cfg(debug_assertions)]
    {
        mutator.jit.entries_in_progress -= 1;
    }
    debug_assert_eq!(
        kind,
        block.get(BlockWord::ExitKind.index()),
        "internal: the exit stub answers the kind it wrote"
    );
    if mutator.jit.counting {
        mutator.jit.statistics.machine_back_edges += block.get(BlockWord::BackEdges.index());
        mutator.jit.statistics.compiled_calls += block.get(BlockWord::CompiledCalls.index());
    }
    kind
}

/// Where the run loop goes on after machine code exited with `kind`, no
/// return, as `block` describes the exit, `innermost` the innermost frame
/// the exit materialized or the entry's: the exit counted, a body that
/// has spent its budget abandoned, and the innermost frame interpreted
/// from here.
#[cfg(ns_jit)]
fn resumption_after_exit(
    mutator: &mut Mutator<'_>,
    block: &EntryBlock,
    kind: u64,
    innermost: ActivationReference,
) -> Resumption {
    use stubs::MachineExit;

    let exit = stubs::decode_exit(block, kind);
    mutator.jit.statistics.exits += 1;
    // Counted on every exit, not only while counting, since the abandonment
    // and the tests read it: a match on the exit's slot, which decodes
    // nothing (`CompiledCode::is_send`), on a path that has just left
    // machine code.
    let at_a_send = match exit {
        MachineExit::PushRefused => true,
        MachineExit::Reexecute { program_counter } => mutator
            .heap
            .activation_code(innermost)
            .is_send(program_counter as usize),
        _ => false,
    };
    if at_a_send {
        mutator.jit.statistics.send_exits += 1;
    }
    // The stress mode enters every body at every push, however often it
    // leaves.
    if matches!(
        exit,
        MachineExit::Reexecute { .. } | MachineExit::PushRefused
    ) && !mutator.jit.install_bodies_exiting_at_once
    {
        let code = mutator.heap.activation_code(innermost);
        if code.jit.note_exit() {
            abandon(mutator, &code);
        }
    }
    // The innermost frame goes on interpreted from here: it no longer
    // stands in machine code, which only the frames below it still do.
    mutator.heap.clear_compiled(innermost);
    match exit {
        MachineExit::Returned { .. } => {
            unreachable!("internal: a return is answered before the exit is decoded")
        }
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

/// The words of the entry block that name what lasts from one entry to
/// the next, with what they name now: the reservations machine code reads
/// and writes, the stubs it reaches through the block -- the exit stub at
/// `exit_stub`, the polymorphic stub and the cut stub -- and the global
/// cache's mask.
#[cfg(ns_jit)]
fn lasting_block_words(mutator: &Mutator<'_>, exit_stub: u64) -> [(stubs::BlockWord, u64); 12] {
    use stubs::BlockWord;
    [
        (
            BlockWord::ObjectSpaceBase,
            mutator.heap.object_space_base_address() as u64,
        ),
        (
            BlockWord::StackBase,
            mutator.heap.activation_stack_base_address() as u64,
        ),
        (
            BlockWord::MachineWords,
            mutator.heap.machine_words_address() as u64,
        ),
        (BlockWord::ExitStub, exit_stub),
        (BlockWord::Cells, mutator.caches.site_cells_address() as u64),
        (
            BlockWord::PolymorphicStub,
            mutator.jit.polymorphic_stub_address(),
        ),
        (
            BlockWord::PolymorphicBlocks,
            mutator.caches.block_cells_address() as u64,
        ),
        (
            BlockWord::GlobalRows,
            mutator.caches.global_rows_address() as u64,
        ),
        (BlockWord::GlobalMask, mutator.caches.global_mask()),
        (
            BlockWord::EdenCursor,
            mutator.heap.eden_cursor_address() as u64,
        ),
        (
            BlockWord::MachineEntries,
            mutator.heap.machine_entries_address() as u64,
        ),
        (BlockWord::CutStub, mutator.jit.cut_stub_address()),
    ]
}

/// Abandons `code`, whose machine code has spent its `EXIT_BUDGET`: its
/// body is settled `ABANDONED`, so that no push and no frame returned to
/// enters it again until its exits decay (`JitState::decay_exits`), and
/// every cell that names its machine entry is written afresh from its
/// entry, which now names the stub that refuses the call. Its machine code
/// is freed at once (`JitState::free_body`), and its bytes reclaimed by the
/// next compaction, so that a revived body is compiled anew: abandonment
/// runs only between entries (`resumption_after_exit`), where no return
/// address into the zone exists, and a frame an exit left stopped in the
/// code resumes interpreted (`resumed`), as every frame stopped in freed
/// code does.
#[cfg(ns_jit)]
fn abandon(mutator: &mut Mutator<'_>, code: &Rc<CompiledCode>) {
    let installed = mutator.jit.installed(code).map(|body| body.start);
    let entry = installed.map(CodeOffset);
    if installed.is_some() {
        mutator.jit.free_body(code.jit.map.get());
    }
    code.jit.settle(ABANDONED);
    mutator.jit.abandoned.push(Rc::downgrade(code));
    mutator.jit.statistics.abandoned_bodies += 1;
    // A block's closure stubs call it no more either.
    mutator.heap.forget_machine_entry(code);
    let Some(zone) = mutator.jit.zone.as_ref() else {
        return;
    };
    if let Some(entry) = entry {
        let address = zone.writer.zone().executable_address(entry) as u64;
        mutator.caches.refresh_cells_entering(address, &mutator.jit);
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
        let offsets = [ns_memory::CodeOffset(64), ns_memory::CodeOffset(80)];
        slot.compiled(7, offsets, 3);
        assert_eq!(
            format!("{slot:?}"),
            "JitSlot::Compiled { zone: 7, offset: 64, first_instruction: 80, map: 3 }"
        );
        assert!(slot.is_armed(), "a push enters every compiled body");
        assert!(slot.is_compiled());
        assert_eq!(format!("{:?}", slot.clone()), "JitSlot::Counting(0)");
        slot.refuse();
        assert!(!slot.is_armed());
        assert_eq!(format!("{slot:?}"), "JitSlot::Refused");
    }

    /// The machine-code tests, where the build emits machine code.
    #[cfg(ns_jit)]
    pub(in crate::jit) mod machine {
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
        pub(in crate::jit) fn body_with_a_loop() -> Rc<CompiledCode> {
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

        pub(in crate::jit) fn enter_twice(state: &mut JitState, code: &Rc<CompiledCode>) {
            note_entry(state, code, FeedbackBase::NOT_ASSIGNED, None);
            note_entry(state, code, FeedbackBase::NOT_ASSIGNED, None);
        }

        /// A body whose code runs straight to its return is compiled, and
        /// entered at a push as at a send.
        #[test]
        fn a_body_that_runs_to_a_return_is_compiled() {
            let mut state = JitState::for_test(1 << 16, false);
            let code = body();
            enter_twice(&mut state, &code);
            assert!(machine_entry(&state, &code).is_some());
            assert!(code.jit.is_armed(), "a push enters it");
            assert!(state.is_entered_here(&code));
            assert!(state
                .interpreter_entry(&code, FeedbackBase::NOT_ASSIGNED, 0)
                .is_some());
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
            assert!(code.jit.is_armed(), "a push enters it");
            assert!(state
                .interpreter_entry(&code, FeedbackBase::NOT_ASSIGNED, 0)
                .is_some());
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
                &[],
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

        /// A body that makes a closure, so that it reifies its frame,
        /// but returns 7 from its first named local before it does.
        pub(in crate::jit) fn body_making_a_closure() -> Rc<CompiledCode> {
            let block = Rc::new(CompiledCode::new(
                CodeKind::Block,
                vec![Instruction::ImplicitReturn { register: 13 }],
                0,
                14,
                0,
                Vec::new(),
                ns_syntax::Span::new(0, 0),
            ));
            let mut code = CompiledCode::new(
                CodeKind::Method,
                vec![
                    Instruction::LoadLiteral {
                        destination: 13,
                        literal: Literal::SmallInteger(7),
                    },
                    Instruction::Return { register: 13 },
                    Instruction::MakeClosure {
                        destination: 13,
                        parameters: Rc::new(Vec::new()),
                        code: block,
                        copied: Vec::new(),
                    },
                    Instruction::Return { register: 13 },
                ],
                0,
                14,
                0,
                Vec::new(),
                ns_syntax::Span::new(0, 0),
            );
            // Register 13 is a named local, which a retained frame keeps.
            code.local_count = 14;
            Rc::new(code)
        }

        /// The return of a frame whose code reifies it divorces the frame
        /// object in machine code -- inline, or through the divorce leaf
        /// for one that retains its registers -- as the pop divorces it,
        /// and leaves the frame unreified, so that the run loop's pop
        /// after it divorces nothing again: the frame the run loop
        /// entered machine code at, reified before, as the interpreter
        /// reifies a frame an exit left.
        #[test]
        fn a_reified_frame_is_divorced_by_its_return_in_machine_code() {
            for retains_registers in [false, true] {
                let mut interpreter = Interpreter::bootstrap_with(crate::Lowering::Optimized);
                interpreter.set_jit_enabled(true);
                interpreter.enter(|mutator| {
                    divorced_by_its_return(mutator, retains_registers);
                });
            }
        }

        fn divorced_by_its_return(
            mutator: &mut crate::interpreter::Mutator<'_>,
            retains_registers: bool,
        ) {
            let code = body_making_a_closure();
            note_entry(&mut mutator.jit, &code, FeedbackBase::NOT_ASSIGNED, None);
            note_entry(&mut mutator.jit, &code, FeedbackBase::NOT_ASSIGNED, None);
            let depth = mutator.heap.activation_depth();
            // A factory's activation, as the body compiled without a method
            // runs: a top-level one is never entered in machine code.
            let class = mutator.builtin_class(crate::types::BuiltinClass::Object);
            let activation = mutator
                .heap
                .push_activation(
                    crate::heap::ActivationHeader {
                        receiver: crate::Value::object(class),
                        current_class: Some(class),
                        kind: crate::types::ActivationKind::Factory(class),
                        method: None,
                        feedback_base: FeedbackBase::NOT_ASSIGNED,
                        code: Rc::clone(&code),
                        then: crate::types::Continuation::Deliver,
                        counted: true,
                    },
                    crate::interpreter::Arguments::NONE,
                    code.registers_above_header(),
                )
                .unwrap_or_else(|_| panic!("the stack holds the frame"));
            let frame = mutator.heap.reify(activation);
            // A program counter the frame stored, which the divorce copies.
            mutator.heap.activation_set_program_counter(activation, 1);
            if retains_registers {
                mutator.heap.retain_registers(activation);
            }
            let leaf_calls = mutator.jit.statistics.leaf_calls;
            let register_base = mutator.heap.activation_register_base(activation);
            let crate::jit::Resumption::Returned { value } =
                crate::jit::entered(mutator, activation, &code, register_base)
            else {
                panic!("the body runs to its return in machine code");
            };
            assert_eq!(
                value,
                Immediate::small_integer(7).expect("a small integer").bits()
            );
            assert_eq!(
                mutator.jit.statistics.leaf_calls - leaf_calls,
                u64::from(retains_registers),
                "only a frame that retains its registers calls the divorce leaf"
            );
            assert!(
                mutator.heap.frame_activation(frame).is_none(),
                "the frame is married no longer"
            );
            assert!(mutator.heap.frame_is_uncontinuable(frame));
            assert_eq!(mutator.heap.frame_sender(frame).map(|_| ()), None);
            assert_eq!(
                mutator.heap.frame_register_shape(frame).0,
                1,
                "the program counter the frame stored"
            );
            let local = mutator.heap.frame_local(frame, 13).as_small_integer();
            if retains_registers {
                assert_eq!(local, Some(7), "the registers were copied out");
            } else {
                assert_eq!(local, None, "no register was copied out");
            }
            // The frame reads as unreified: reifying it again makes another
            // frame object, which the pop divorces in its stead.
            let again = mutator.heap.reify(activation);
            assert!(!again.same(frame), "the return cleared the reified word");
            mutator.heap.pop_activation(activation);
            assert_eq!(mutator.heap.activation_depth(), depth);
        }

        /// A body's closure site lies in the zone among the body's data
        /// words: past its last instruction, on a cache line no
        /// instruction shares, before the next body's code, and zeroed at
        /// install, so that its first run fills it.
        #[test]
        fn closure_sites_live_with_their_body() {
            let mut state = JitState::for_test(1 << 16, true);
            let making = body_making_a_closure();
            let next = body();
            enter_twice(&mut state, &making);
            enter_twice(&mut state, &next);
            assert_eq!(state.statistics().compiled_bodies, 2);
            let (first, second) = (&state.bodies[0], &state.bodies[1]);
            let alignment = crate::jit::macro_assembler::BODY_WORDS_ALIGNMENT;
            assert!(first.words < first.end, "the body has data words");
            assert_eq!(first.words_end - first.words, 24, "one site's three words");
            assert_eq!(first.words % alignment, 0, "the words start a line");
            assert_eq!(first.end % alignment, 0, "the words end a line");
            let last_instruction = first
                .map
                .starts
                .iter()
                .map(|offset| first.start + offset)
                .max()
                .expect("an instruction");
            assert!(last_instruction < first.words, "past the body's code");
            assert!(first.end <= second.start, "before the next body's");
            assert_eq!(
                second.words, second.end,
                "a body without sites has no words"
            );
            let zone = state.zone.as_ref().expect("a zone");
            let site = zone
                .writer
                .bytes(first.words..first.words + 24)
                .expect("committed");
            assert!(site.iter().all(|byte| *byte == 0), "zeroed at install");
        }

        /// A zone with room for a body's code but not for its closure
        /// site refuses the body, as a zone too full for its code does:
        /// sites take their room from the zone's.
        #[test]
        fn a_zone_without_room_for_a_body_s_sites_refuses_it() {
            let mut probe = JitState::for_test(1 << 16, true);
            let making = body_making_a_closure();
            enter_twice(&mut probe, &making);
            let installed = &probe.bodies[0];
            let (end, words) = (installed.end as usize, installed.words as usize);
            for (bytes, compiles) in [(words + 8, false), (end - 1, false), (end, true)] {
                let mut state = JitState::for_test(bytes, true);
                let code = body_making_a_closure();
                enter_twice(&mut state, &code);
                assert_eq!(
                    machine_entry(&state, &code).is_some(),
                    compiles,
                    "a zone of {bytes} bytes, the body's words at {words}, its end at {end}"
                );
            }
        }

        /// `call:` sends `value:value:value:`, whose closure stub copies
        /// the three arguments past the send's slots, which end one word
        /// below `call:`'s window end. With every push seating the
        /// ordinary end at the pushed frame's window end -- where the
        /// committed stack may end -- the copies would reach past it: the
        /// stub declines before it writes them, the interpreter's push of
        /// the block grows the stack, and `run` answers what it answers
        /// with the JIT off.
        const AT_THE_ORDINARY_END: &str = indoc::indoc! {"
            class Subject = Object new () (
              call: block = (
                ^block value: 1 value: 2 value: 3
              )
              public run = (
                |
                  index ::= 0.
                  sum ::= 0.
                |
                [index < 20] whileTrue: [
                  sum:: sum + (self call: [:first :second :third | first + second + third + index]).
                  index:: index + 1.
                ].
                ^sum
              )
            ) : ()
        "};

        fn run_at_the_ordinary_end(jit: bool) -> (i64, crate::jit::JitStatistics, u64) {
            let module = ns_syntax::parse(AT_THE_ORDINARY_END).expect("parse");
            let resolved = crate::resolve(
                module,
                std::rc::Rc::from(AT_THE_ORDINARY_END),
                crate::SourceOrigin::File(std::path::PathBuf::from("edge.ns")),
            )
            .expect("resolve");
            let mut interpreter = Interpreter::bootstrap_with(crate::Lowering::Optimized);
            interpreter.set_jit_enabled(jit);
            let class = interpreter.load_module(&resolved);
            let subject = match interpreter.instantiate(&class, &[]) {
                Ok(subject) => subject,
                Err(_) => panic!("instantiate"),
            };
            interpreter.enter(|mutator| mutator.heap.set_seat_ordinary_end_at_top(true));
            let mut answer = 0;
            let mut before = interpreter.jit_statistics();
            for round in 0..4 {
                if round == 3 {
                    before = interpreter.jit_statistics();
                }
                let Ok(result) = interpreter.send(&subject, crate::Symbol::intern("run"), &[])
                else {
                    panic!("run raised");
                };
                answer = interpreter.enter(|mutator| {
                    mutator
                        .fetch(&result)
                        .as_small_integer()
                        .expect("a small integer")
                });
            }
            let after = interpreter.jit_statistics();
            let mut statistics = after;
            statistics.exits = after.exits - before.exits;
            statistics.machine_entries = after.machine_entries - before.machine_entries;
            let disturbances = interpreter.enter(|mutator| mutator.heap.seat_disturbances());
            (answer, statistics, disturbances)
        }

        /// A closure stub's copies of a block row's arguments, which lie
        /// past the sender's window before the block's prologue checks its
        /// frame's room, are made only below the ordinary end: at it the
        /// call declines, the sentinel each seat writes right above the
        /// end stays whole, and the program answers the interpreter's
        /// value.
        #[test]
        fn a_closure_stub_declines_copies_past_the_ordinary_end() {
            let (expected, _, interpreted_disturbances) = run_at_the_ordinary_end(false);
            assert_eq!(expected, 20 * 6 + 190);
            assert_eq!(
                interpreted_disturbances, 0,
                "the interpreter writes nothing past it"
            );
            let (answer, statistics, disturbances) = run_at_the_ordinary_end(true);
            assert_eq!(answer, expected, "the interpreter's answer");
            assert!(statistics.machine_entries > 0, "machine code ran");
            assert!(statistics.exits > 0, "the calls at the edge are refused");
            assert_eq!(
                disturbances, 0,
                "no word past a seated ordinary end is written before the stack grows"
            );
        }

        /// A full zone refuses no body for good: the body that found no
        /// room counts its entries again, a compaction is asked for, and
        /// until one frees room a body that found no room counts its
        /// entries but compiles nothing.
        #[test]
        fn a_full_zone_asks_for_a_compaction() {
            // Three pages: the stubs take most of two.
            let mut state = JitState::for_test(12_288, true);
            let bodies: Vec<Rc<CompiledCode>> = (0..200).map(|_| body()).collect();
            for code in &bodies {
                enter_twice(&mut state, code);
            }
            let statistics = state.statistics();
            assert!(statistics.compiled_bodies >= 1);
            assert!(statistics.compiled_bodies < 200, "the zone fills");
            assert_eq!(statistics.refused_bodies, 0, "no room refuses nothing");
            assert!(state.compaction_requested());
            assert!(state.zone_exhausted);
            let last = bodies.last().expect("a body");
            let first_refused = bodies
                .iter()
                .find(|code| machine_entry(&state, code).is_none())
                .expect("a body found no room");
            assert_eq!(
                format!("{:?}", first_refused.jit),
                "JitSlot::Counting(0)",
                "back to counting"
            );
            assert_eq!(
                format!("{:?}", last.jit),
                "JitSlot::Counting(0)",
                "tried once while exhausted, and found no room either"
            );
            assert_eq!(machine_entry(&state, last), None);
            let compiled = state.statistics().compiled_bodies;
            enter_twice(&mut state, last);
            assert_eq!(state.statistics().compiled_bodies, compiled);
            assert_eq!(
                format!("{:?}", last.jit),
                "JitSlot::Counting(2)",
                "known not to fit: due, held at the entry that compiles"
            );
            state.note_full_collection();
            state.compaction_requested = false;
            note_entry(&mut state, last, FeedbackBase::NOT_ASSIGNED, None);
            assert!(state.compaction_requested(), "the zone is still full");
        }

        /// An exhausted zone asks for a compaction again once it has held
        /// `COMPILES_HELD_BEFORE_RETRY` compiles, and twice as many after
        /// each further compaction in a row that left it exhausted: its
        /// end waits on entries, never on allocation or time.
        #[test]
        fn an_exhausted_zone_asks_again_after_its_held_compiles() {
            let mut state = JitState::for_test(12_288, true);
            let bodies: Vec<Rc<CompiledCode>> = (0..200).map(|_| body()).collect();
            for code in &bodies {
                enter_twice(&mut state, code);
            }
            assert!(state.zone_exhausted);
            let last = bodies.last().expect("a body");
            // Due, and known not to fit: every entry from now on is held.
            enter_twice(&mut state, last);
            for fruitless in 1..=3 {
                // As a fruitless compaction leaves it.
                state.compaction_requested = false;
                state.held_compiles = 0;
                state.fruitless_compactions = fruitless;
                let wait = crate::jit::zone::compiles_held_before_retry(fruitless);
                assert_eq!(
                    wait,
                    crate::jit::zone::COMPILES_HELD_BEFORE_RETRY << (fruitless - 1)
                );
                for _ in 1..wait {
                    note_entry(&mut state, last, FeedbackBase::NOT_ASSIGNED, None);
                }
                assert!(!state.compaction_requested(), "one held compile short");
                note_entry(&mut state, last, FeedbackBase::NOT_ASSIGNED, None);
                assert!(state.compaction_requested(), "asked again");
                assert!(state.zone_exhausted, "but compiling nothing until then");
            }
        }

        /// A zone exhausted for a large body still compiles a small one that
        /// fits the room it has, while a body known not to fit is held.
        #[test]
        fn an_exhausted_zone_compiles_a_body_that_fits() {
            let mut state = JitState::for_test(65_536, true);
            let first = body();
            enter_twice(&mut state, &first);
            assert!(machine_entry(&state, &first).is_some());
            let room = {
                let zone = state.zone.as_ref().expect("a zone");
                zone.limit - zone.next_offset
            };
            assert!(room > 4_096, "room left: {room}");
            // As a compile of a body larger than the room left leaves it.
            state.zone_exhausted = true;
            state.wanted_bytes = room + 1;
            let large = body();
            large.jit.needed.set(room + 1);
            enter_twice(&mut state, &large);
            assert_eq!(machine_entry(&state, &large), None, "held: it does not fit");
            assert_eq!(state.held_compiles, 1);
            let small = body();
            enter_twice(&mut state, &small);
            assert!(
                machine_entry(&state, &small).is_some(),
                "compiled: it fits the room left"
            );
            assert!(state.zone_exhausted, "still exhausted for the large one");
            assert!(!state.compaction_requested(), "and asking for nothing");
        }

        /// A compiled block pushed while the JIT is off is refused for
        /// good, and its machine entry row, which the closure stubs jump
        /// through, names its machine code no more once the JIT is on
        /// again: the switch cleared it, and the refused body's push does
        /// not write it afresh.
        #[test]
        fn a_block_refused_while_the_jit_is_off_keeps_no_machine_entry_row() {
            let mut interpreter = Interpreter::bootstrap_with(crate::Lowering::Optimized);
            interpreter.set_jit_enabled(true);
            let block = Rc::new(CompiledCode::new(
                CodeKind::Block,
                vec![Instruction::ImplicitReturn { register: 13 }],
                0,
                14,
                0,
                Vec::new(),
                ns_syntax::Span::new(0, 0),
            ));
            let feedback_base = FeedbackBase::from_raw(0);
            let id = interpreter.enter(|mutator| {
                note_entry(&mut mutator.jit, &block, feedback_base, None);
                note_entry(&mut mutator.jit, &block, feedback_base, None);
                assert!(machine_entry(&mutator.jit, &block).is_some(), "compiled");
                let id = mutator.heap.code_id(&block);
                super::super::note_block_entry(mutator, id, &block);
                assert!(
                    mutator.heap.machine_entry_recorded(id),
                    "its push wrote the row"
                );
                id
            });
            interpreter.set_jit_enabled(false);
            interpreter.enter(|mutator| {
                // The push of the block while the JIT is off.
                note_entry(&mut mutator.jit, &block, feedback_base, None);
                super::super::note_block_entry(mutator, id, &block);
                assert_eq!(format!("{:?}", block.jit), "JitSlot::Refused");
            });
            interpreter.set_jit_enabled(true);
            interpreter.enter(|mutator| {
                super::super::note_block_entry(mutator, id, &block);
                assert!(
                    !mutator.heap.machine_entry_recorded(id),
                    "no row names the refused body's machine code"
                );
            });
        }
    }
}
