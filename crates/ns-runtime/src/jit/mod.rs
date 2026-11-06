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
//! build script sets `cfg(ns_jit)` -- x64, and AArch64 on Linux and
//! Apple's platforms (`build.rs`); elsewhere `available` answers `false` and every body
//! stays interpreted.
#![cfg_attr(
    not(ns_jit),
    expect(
        dead_code,
        reason = "without a JIT no body is compiled, so nothing installs code or maps it"
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
pub(crate) mod optimizing;
#[cfg(ns_jit)]
pub(crate) mod sends;
#[cfg(ns_jit)]
pub(crate) mod stubs;
#[cfg(ns_jit)]
pub(crate) mod targets;
pub(crate) mod zone;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use ns_memory::{CodeOffset, CodeWriter, EntryBlock};

use crate::bytecode::CompiledCode;
use crate::hash::IdentityMap;
use crate::heap::{ActivationReference, ClassReference, CodeId, Heap, RegisterBase, Value};
use crate::interpreter::{FeedbackBase, Mutator, DEPTH_RESERVE, MAX_ACTIVATION_DEPTH};
use crate::mixin::{DispatchId, MixinApplication, MixinMethod};

/// The code zone's size unless a test asks for another. A first guess
/// within the 128 MiB bound: the size is chosen by measurement once
/// bodies run as machine code (`docs/OPEN-WORK.md`, road step 8).
pub(crate) const DEFAULT_ZONE_BYTES: usize = 16 << 20;

/// The entry at which a body is compiled: its second, as Cog compiles a
/// method found in its method cache, so that code run once -- most of a
/// program's initialization -- is never compiled
/// (`docs/design/execution.md`, "When a body is compiled").
const COMPILE_AT_ENTRY: u32 = 2;

/// The interpreted backward jump at which a body without machine code is
/// compiled, and its frame switched into the code at the loop's head: its
/// 40th, Cog's `MinBackwardJumpCountForCompile`
/// (`prior-art/opensmalltalk-vm/src/spur64.sista/cointerp.c:218`, `:2759`,
/// counted at `:8379-8409`), so that a loop in a body entered once -- a
/// benchmark's main loop -- runs compiled (`docs/design/execution.md`,
/// "When a body is compiled").
const COMPILE_AT_BACK_JUMP: u32 = 40;

/// Every body starts at a multiple of this many bytes of the zone, the
/// processor's instruction fetch block.
const BODY_ALIGNMENT: u32 = 16;

/// The instructions a body runs in baseline machine code -- its length at
/// each entry, a loop's at each back jump -- before its optimization is
/// requested (`heap/optimization_budgets.rs`). Chosen from a sweep over
/// 30,000, 60,000, 120,000 and 240,000 (`docs/design/execution.md`, "What
/// the optimizing tier's foundation moved"): no budget moved a program
/// beyond the spread of one build, and the largest left DeltaBlueTon80,
/// whose optimized code runs level with its baseline code, the closest
/// to `master` and compiles the fewest bodies; a reduced sweep at the
/// final code kept it so.
pub(crate) const OPTIMIZATION_BUDGET: i64 = 240_000;

/// The least budget a body is left with when a send of it filled its
/// site's cache after a miss, so that its optimization waits for its
/// feedback to settle: a sixth of `OPTIMIZATION_BUDGET`, the ratio of the
/// first guesses, kept as the budget doubled. The wait decides when a body
/// is compiled, and so which fallback sites have filled when the compile
/// reads them (`optimizing::snapshot::Snapshot::fallback_taken`): the
/// code's layout, never its semantics. Swept over 0, 10,000, 40,000,
/// 120,000 and 240,000 (`docs/design/execution.md`, "What the optimizing
/// tier's foundation moved"): Mandelbrot, NBody and Json retired the same
/// instructions to 0.05% at every value -- each compiled the same code --
/// and no value separated from another.
pub(crate) const BUDGET_AFTER_CACHE_CHANGE: i64 = OPTIMIZATION_BUDGET / 6;

/// The native stack an entry into machine code needs left: a return
/// address's stack slot for every activation the depth limit and its
/// reserve count, since compiled frames keep theirs on the native stack,
/// and 64 KiB for the deepest leaf call below the innermost
/// (`docs/design/execution.md`, "The native stack carries the return
/// addresses"). A thread with less runs interpreted.
pub(crate) const NATIVE_STACK_NEED: usize =
    (MAX_ACTIVATION_DEPTH + DEPTH_RESERVE) * RETURN_ADDRESS_BYTES + 64 * 1024;

/// The bytes a compiled frame's return address takes on the native stack,
/// the host convention's stack slot.
#[cfg(ns_jit)]
const RETURN_ADDRESS_BYTES: usize = macro_assembler::Abi::host()
    .stack_slot_bytes()
    .unsigned_abs() as usize;

/// No machine code runs, so no frame keeps a return address on the
/// native stack.
#[cfg(not(ns_jit))]
const RETURN_ADDRESS_BYTES: usize = 0;

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

/// The word a literal is when `make_value` makes it without allocating:
/// `nil`, a Boolean, a `SmallInteger` and an immediate float. `None` for
/// every other literal. The baseline's load and the optimizing compiler's
/// constants both ask it, so the two tiers agree on which literal is a
/// word known at compile time.
pub(crate) fn immediate_literal(
    literal: &crate::bytecode::Literal,
) -> Option<crate::heap::Immediate> {
    use crate::bytecode::Literal;
    use crate::heap::Immediate;
    match literal {
        Literal::Nil => Some(Immediate::NIL),
        Literal::Boolean(boolean) => Some(Immediate::boolean(*boolean)),
        Literal::SmallInteger(integer) => Immediate::small_integer(*integer),
        Literal::Float(float) => Immediate::small_float(*float),
        Literal::String(_)
        | Literal::Symbol(_)
        | Literal::LargeInteger(_)
        | Literal::UnresolvedIdentifier(_)
        | Literal::BodyTooLarge(_) => None,
    }
}

/// What the JIT has done in one interpreter, counted in every build
/// (`Interpreter::jit_statistics`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JitStatistics {
    /// Bodies compiled to machine code.
    pub compiled_bodies: u64,
    /// Twins compiled: the reifying compile of a body, made when a
    /// transfer of its lean code or a resumption of a reified frame
    /// first needs it (`docs/design/execution.md`, "A frame reifies only
    /// in its body's twin"). Not counted in `compiled_bodies`.
    pub compiled_twins: u64,
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
    /// Compiled calls refused before the callee was pushed
    /// (`ExitKind::PushRefused`): by the callee's prologue, at the depth
    /// limit, at a safepoint requested, or where the activation stack's
    /// ordinary room cannot hold the frame -- never for a frame past the
    /// ordinary end alone, which the growth leaf makes room for
    /// (`jit::leaves::grow_stack`) -- and by the miss stub and the
    /// refusing stub, for a target machine code does not run.
    pub pushes_refused: u64,
    /// Pushes of machine code past the activation stack's ordinary end
    /// the growth leaf made room for, and those it refused, the ordinary
    /// room unable to hold the frame (`jit::leaves::grow_stack`).
    pub stack_growths: u64,
    pub stack_growth_refusals: u64,
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
    /// reclaims their bytes. A twin freed with its body is counted in
    /// `freed_twins` instead.
    pub freed_bodies: u64,
    /// Twins whose machine code was freed, each with its lean body
    /// (`jit::zone`).
    pub freed_twins: u64,
    /// Bytes of machine code and data words the compactions slid down.
    pub moved_bytes: u64,
    /// Abandoned bodies revived, which count their entries again: their
    /// exit count halved below `REVIVAL_EXITS` by the scavenges and full
    /// collections (`JitState::decay_exits`), or their interpreted entries
    /// since their abandonment reaching `revival_entries`
    /// (`JitState::note_abandoned_entry`).
    pub revived_bodies: u64,
    /// Pushes of an abandoned body the run loop handed to `entered`, out
    /// of line: only the push that revives it, since every other push of
    /// it counts its entry inline (`JitSlot::push_is_armed`).
    pub abandoned_entries_out_of_line: u64,
    /// Entries into machine code at an interpreted backward jump, the
    /// frame switched in place at the loop's head (`at_back_edge`).
    pub back_edge_switches: u64,
    /// Interpreted backward jumps that ended the run loop's stretch to
    /// come into machine code there (`vm::Leave::BackEdge`): only those
    /// `back_edge_ready` found ready to switch, so that a body that cannot
    /// switch never leaves its stretch at every turn.
    pub back_edge_stretch_exits: u64,
    /// Bodies compiled at their `COMPILE_AT_BACK_JUMP`th interpreted
    /// backward jump rather than at an entry (`back_edge_ready`).
    pub back_edge_compiles: u64,
    /// Entries into machine code at a loop's head right after the
    /// safepoint a backward jump stopped at -- a machine-code poll's exit
    /// or an interpreted jump's test -- so that a loop that allocates goes
    /// on in machine code after each scavenge (`vm::run`). Each is one of
    /// `back_edge_switches`, and none of `back_edge_stretch_exits`.
    pub safepoint_resumptions: u64,
    /// Wall time of every compile of a body or a twin, assembly and
    /// installation included, refused ones too, in nanoseconds: what a
    /// compiler thread of its own would take off the mutator.
    pub compile_nanoseconds: u64,
    /// Bodies whose optimization budget ran out in baseline machine code,
    /// each requested once per key while its request is pending
    /// (`JitState::optimization_requests`).
    pub optimization_requests: u64,
    /// Bodies compiled to optimized machine code, each customized copy
    /// of a method's body among them.
    pub optimized_bodies: u64,
    /// The customized copies of method bodies compiled
    /// (`CustomizedCopy`): of `optimized_bodies`.
    pub customized_copies: u64,
    /// Calls through a method body's dispatching entry -- a `super` or
    /// private lexical send's, and an interpreted send's (`jit::called`),
    /// once the body has a customized copy
    /// (`targets::emit_dispatching_entry`) -- that entered the copy whose
    /// row matched: counted by the bodies compiled while the JIT counts, as
    /// `compiled_calls` is.
    pub dispatched_calls: u64,
    /// Exits of optimized machine code.
    pub optimized_exits: u64,
    /// Bodies whose optimized machine code was abandoned for its exits.
    pub abandoned_optimized_bodies: u64,
    /// Wall time of every optimizing compile, in nanoseconds, as
    /// `compile_nanoseconds` counts the baseline's.
    pub optimize_nanoseconds: u64,
    /// Frames the exit's walk materialized that stood in optimized code:
    /// a frame whose call returns into optimized code, or the innermost
    /// one, which left it.
    pub optimized_frames_materialized: u64,
    /// Frames returned to (`resumed`) of a body with optimized code, which
    /// go on in its baseline code: one of `resumed_entries`.
    pub optimized_bodies_resumed: u64,
    /// The twin leaf's calls from optimized code's `MakeClosure`, whose
    /// transfer found its baseline body's twin not compiled yet.
    pub optimized_twin_requests: u64,
    /// Exits of optimized code the deopt stress mode forced at an
    /// interrupt point, and the invalidate stress mode at the last point
    /// of its countdown: of `optimized_exits`.
    pub forced_exits: u64,
    /// The largest countdown a forced exit came at: how many interrupt
    /// points optimized code had passed since the safepoint that set it,
    /// the exiting one included.
    pub deepest_forced_exit: u64,
    /// Exits of optimized code at a failed speculation
    /// (`stubs::ExitKind::Trap`): of `optimized_exits`, counted against no
    /// exit budget.
    pub traps: u64,
    /// Optimized copies made not entrant for their traps (`count_trap`):
    /// a second trap at a site, or `TRAP_LIMIT` traps of one copy.
    pub made_not_entrant: u64,
    /// Keys -- or a body's one uncustomized copy -- left in baseline code
    /// for good, their copy past `LAST_VERSION` made not entrant or
    /// abandoned all the same (`retire_optimized`).
    pub refused_for_good: u64,
    /// Optimized copies freed because what they were compiled against
    /// changed: the copies keyed on an application an install rebuilt
    /// (`invalidate_applications`) or whose id a full collection reclaimed
    /// (`forget_freed_copies`), every copy at a bulk forward
    /// (`invalidate_all_optimized`), and those the invalidate stress mode's
    /// rounds free (`invalidate_optimized_for_stress`). Neither a trap nor
    /// an exit: no version is raised for them.
    pub invalidated_copies: u64,
}

impl JitStatistics {
    /// Adds the time since `started` to `compile_nanoseconds`.
    fn count_compile_time(&mut self, started: std::time::Instant) {
        let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.compile_nanoseconds = self.compile_nanoseconds.saturating_add(elapsed);
    }

    /// Counts a freed installed body: in `freed_twins` for a twin, in
    /// `freed_bodies` for any other.
    fn count_freed(&mut self, twin: bool) {
        if twin {
            self.freed_twins += 1;
        } else {
            self.freed_bodies += 1;
        }
    }
}

/// What a body's `JitSlot` says once it is compiled, and so entered at a
/// send, a push and a frame returned to. Below every abandoned state, so
/// that `push_is_armed`'s first compare tells the bodies a push hands to
/// `entered` from the others.
const COMPILED: u32 = FIRST_ABANDONED - 1;
/// The lowest of the states of a body the JIT abandoned for its exits
/// (`abandon`): neither compiled nor entered in machine code until it is
/// revived -- by the scavenges and full collections halving its exit count
/// below `REVIVAL_EXITS` (`JitState::decay_exits`), or by its interpreted
/// entries adding up to the wait its abandonment set
/// (`revival_entries`). An abandoned body's state counts those entries:
/// `abandon` settles it `EXITS_AT_ONCE` less its wait, and each
/// interpreted push adds one inline (`JitSlot::push_is_armed`) until it
/// stands at `LAST_ABANDONED`, whose push is the one that revives it, the
/// only one handed to `entered` -- an out-of-line call on every push of an
/// abandoned body cost a program that keeps one abandoned about 20 ns a
/// push (`docs/design/execution.md`, "The baseline JIT's foundation").
const FIRST_ABANDONED: u32 = EXITS_AT_ONCE - MAXIMUM_REVIVAL_WAIT;
/// The state of an abandoned body whose next interpreted entry revives it
/// (`FIRST_ABANDONED`).
const LAST_ABANDONED: u32 = EXITS_AT_ONCE - 1;
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
/// Abandonment is not for good. Every scavenge and full collection halves
/// the count (`JitState::decay_exits`), so that in a run that allocates the
/// budget bounds the exits of a stretch of the run rather than of its
/// lifetime; and an abandoned body is revived once it has been entered
/// interpreted `revival_entries` times, so that a run that allocates
/// nothing is not left interpreted either. Either way a rare exit does not
/// end a long run interpreted.
const EXIT_BUDGET: u32 = 64;

/// The exit count below which an abandoned body is revived: it counts its
/// entries again, and is compiled at its `COMPILE_AT_ENTRY`th. Half the
/// budget, so that a body abandoned at `EXIT_BUDGET` needs two halvings --
/// two scavenges or full collections -- and a body that always exits is
/// compiled at most once per two of them.
const REVIVAL_EXITS: u32 = EXIT_BUDGET / 2;

/// The interpreted entries after which a body abandoned for the first time
/// is revived whatever the collector did (`revival_entries`). An exit and
/// the entry that resumes its frame cost about a hundred interpreted
/// instructions (`EXIT_BUDGET`), so a body that always exits wastes at most
/// `EXIT_BUDGET` of them -- about 6,400 instructions' worth -- and one
/// compile per revival; against 64 times the budget, 4,096 interpreted
/// entries of at least a few instructions each, that is under two
/// instructions' worth per entry at the first revival, and it halves at
/// each later one while the entries double, up to the cap.
const REVIVAL_ENTRIES: u32 = 64 * EXIT_BUDGET;

/// The most doublings of `REVIVAL_ENTRIES`: a wait of at most 65,536
/// interpreted entries. Without the cap a body that exits once in N calls,
/// in a run that allocates nothing, ran 64·N calls in machine code between
/// waits that kept doubling, so that a long run ended mostly interpreted;
/// capped, its interpreted share stays below 65,536 / (65,536 + 64·N) --
/// about half for an exit in a thousand calls, a tenth for one in ten
/// thousand -- while a body that always exits still compiles at most once
/// per 65,536 entries once the cap is reached.
const MAXIMUM_REVIVAL_DOUBLINGS: u32 = 4;

/// The longest wait for a revival by entries, 65,536: the most abandoned
/// states a slot has (`FIRST_ABANDONED`).
const MAXIMUM_REVIVAL_WAIT: u32 = REVIVAL_ENTRIES << MAXIMUM_REVIVAL_DOUBLINGS;

/// The traps an optimized copy may take before it is made not entrant
/// (`count_trap`), though no site of it trapped twice: Strongtalk's
/// `UncommonRecompileLimit`, measured and kept: 3 and 10 changed nothing
/// (`docs/design/execution.md`, "What speculation moved"). A copy whose
/// traps are all at sites never seen to trap is still recompiled, so
/// that one trap at each of many sites does not run on in code whose
/// feedback is stale everywhere.
pub(crate) const TRAP_LIMIT: u8 = 5;

/// The last version of a copy compiled with traps
/// (`optimizing::Speculation::at_version`): its recompile after one more
/// failure compiles none, and so can never be made not entrant again,
/// and an abandonment of that one leaves the key in baseline code for
/// good (`retire_optimized`). Strongtalk's `MaxVersions`, as
/// `TRAP_LIMIT` is its limit, and measured and kept as it is. Public,
/// since it bounds what `JitStatistics::made_not_entrant` counts of one
/// copy.
pub const LAST_VERSION: u8 = 3;

/// The interpreted entries a body abandoned for the `abandonments`th time
/// waits before it is revived: `base` -- `REVIVAL_ENTRIES` but in a test
/// (`JitState::set_revival_entries`) -- doubled at each abandonment after
/// the first, at most `MAXIMUM_REVIVAL_DOUBLINGS` times, as an exhausted
/// zone's retries back off (`zone::compiles_held_before_retry`), and never
/// past `MAXIMUM_REVIVAL_WAIT`. The scavenges and full collections halve
/// the abandonments (`JitState::decay_exits`), so that the wait shrinks
/// again in a run that allocates.
fn revival_entries(base: u32, abandonments: u32) -> u32 {
    let doublings = abandonments
        .saturating_sub(1)
        .min(MAXIMUM_REVIVAL_DOUBLINGS);
    base.saturating_mul(1 << doublings)
        .clamp(1, MAXIMUM_REVIVAL_WAIT)
}

/// A body's state in the JIT: *counting* its entries (`state` below
/// `COMPILED`), *compiled* (`COMPILED`; `zone` names the zone its machine
/// code lies in, `offset` where, `map` its program-counter map), *exiting
/// at once* (`EXITS_AT_ONCE`: its code would exit before it reaches a
/// return, a compiled send or a backward jump, so entering it would cost
/// every push more than the few instructions it runs), *abandoned*
/// (`FIRST_ABANDONED` to `LAST_ABANDONED`, until it is revived, the state
/// counting its interpreted entries) or *refused* (`REFUSED`). A push
/// hands only a body counting or compiled to `entered`, so the other
/// three are the top values, which one compare tells from the others
/// (`push_is_armed`).
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
    /// How many times the JIT abandoned the body (`abandon`), which
    /// doubles the interpreted entries its next revival waits for
    /// (`revival_entries`).
    abandonments: Cell<u32>,
    /// Whether the compiler refused the body's twin for a reason no
    /// compaction removes -- too large for the zone, or unassembled -- so
    /// that it is never assembled again: the body is refused for good at
    /// the next exit or resumption that needed it (`refuse_for_its_twin`),
    /// as a reifying body so refused was before twins.
    twin_refused: Cell<bool>,
    /// The body's interpreted backward jumps while it has no machine code
    /// in this zone, which compile it at `JitState::compile_at_back_jump`
    /// (`back_edge_ready`): the body's, not a frame's, as its entries are.
    /// Once it has machine code, the back jumps since a frame of it could
    /// not switch into it, 0 while none failed.
    back_jumps: Cell<u32>,
    /// The body's optimized code, by its index in `bodies`, `NO_BODY` for
    /// none: set only while the slot's baseline code is `COMPILED` in
    /// this zone, beside which it lives (`optimize_requested`), so that
    /// every change of the baseline code's state forgets it.
    optimized: Cell<u32>,
    /// The version of the body's one uncustomized copy, as a copy's row
    /// holds its key's (`CustomizedCopy::version`).
    optimized_version: Cell<u8>,
    /// The traps the body's one uncustomized copy took, as a copy's row
    /// counts its key's (`CustomizedCopy::traps`).
    optimized_traps: Cell<u8>,
    /// Whether the optimizing compiler refused the body for a reason no
    /// compaction removes -- too large for the zone, or unassembled -- or
    /// its uncustomized copy failed past `LAST_VERSION`
    /// (`JitSlot::raise_version`), so that it is never requested nor
    /// compiled again: a recompile of its
    /// baseline code, after an abandonment or a compaction that freed it,
    /// arms a full budget, which would otherwise ask for the same refusal
    /// once per budget for the rest of the run. Kept, as `twin_refused`
    /// is, across every change of the baseline code's state.
    optimized_refused: Cell<bool>,
    /// A method's body's optimized copies, one row per key it was
    /// requested for (`CustomizationKey`), each beside the slot's baseline
    /// code as `optimized` is: a row's `body` is set only while the
    /// baseline code is `COMPILED` in this zone, so that every change of
    /// its state forgets it, while the row itself stays, so that what it
    /// says of its key -- its version, its traps, its refusal -- outlives
    /// its code. Empty for every other body, whose one copy is
    /// `optimized` (`docs/design/execution.md`, "Speculation", "The key and
    /// the copies").
    copies: RefCell<Vec<CustomizedCopy>>,
}

/// What `JitSlot::optimized` and a copy's row hold for a body without
/// optimized code.
const NO_BODY: u32 = u32::MAX;

/// What a method body's optimized copy is compiled for and entered
/// where: the receiver's application, by its instances id, and the
/// application the body is entered at -- the current class's, which the
/// call writes into the frame -- by its instances id too. One body runs
/// at two levels of one receiver's chain when a mixin is applied twice in
/// it, so the receiver alone does not tell its copies apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CustomizationKey {
    pub(crate) receiver: DispatchId,
    pub(crate) entered: DispatchId,
}

impl CustomizationKey {
    /// The key of an activation on `receiver` at `current_class`; `None`
    /// where either has no application with a minted id -- a class object
    /// only a crafted image makes -- which no copy is made for.
    fn of<'gc>(
        heap: &Heap<'gc>,
        receiver: Value<'gc>,
        current_class: Option<ClassReference<'gc>>,
    ) -> Option<CustomizationKey> {
        let receiver = heap.dispatch_id(receiver);
        let entered = heap.class_dispatch_ids(current_class?).instances;
        let minted = |id: DispatchId| id.bits() >= DispatchId::FIRST_MINTED;
        (minted(receiver) && minted(entered)).then_some(CustomizationKey { receiver, entered })
    }
}

/// A method body's row for one key (`JitSlot::copies`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct CustomizedCopy {
    pub(crate) key: CustomizationKey,
    /// The copy's optimized code, by its index in `bodies`; `NO_BODY` while
    /// it has none -- requested and not yet installed, freed, or refused.
    pub(crate) body: u32,
    /// How many times the key's copy was made not entrant for its traps
    /// or abandoned for its exits (`retire_optimized`): what its next
    /// compile waits for (`optimization_budget_after`) and whether it
    /// speculates (`optimizing::Speculation::at_version`). Never decays,
    /// so that a copy that keeps failing reaches its last version.
    pub(crate) version: u8,
    /// The traps the key's current copy took, from none at each install,
    /// halved at every decay (`JitSlot::decay_exits`): at `TRAP_LIMIT` the
    /// copy is made not entrant.
    pub(crate) traps: u8,
    /// Whether the optimizing compiler refused the key's copy for a reason
    /// no compaction removes, so that the key is never requested again.
    pub(crate) refused: bool,
    /// The superclass steps from the receiver's class to the class the
    /// key's body is entered at, `None` where the walk did not find it: a
    /// cell guarded by the receiver's id names the copy whose key its
    /// target's level reaches (`entry_of_target`), since a cell knows the
    /// receiver's application by its id alone, and the entered
    /// application only as a level above it.
    pub(crate) level: Option<u16>,
    /// The application word of the class the key's body is entered at
    /// (`Heap::class_application_word`), read off the frame whose spend
    /// made the row: what the dispatching stub compares a `super` or
    /// private lexical send's current class with (`heap/optimized_copies.rs`).
    pub(crate) entered_word: u64,
}

/// A body whose optimization budget ran out, for the optimizing compile
/// at the next safepoint (`optimize_requested`): held weakly, as the
/// installed bodies are, with the key of the frame whose spend ran it out,
/// `None` for a body that is not customized, and the application the
/// frame was entered at.
pub(crate) struct OptimizationRequest {
    pub(crate) code: Weak<CompiledCode>,
    pub(crate) key: Option<CustomizationKey>,
    pub(crate) entered: Weak<MixinApplication>,
}

impl OptimizationRequest {
    /// The request of `code`'s one copy, which is not customized.
    pub(crate) fn uncustomized(code: &Rc<CompiledCode>) -> OptimizationRequest {
        OptimizationRequest {
            code: Rc::downgrade(code),
            key: None,
            entered: Weak::new(),
        }
    }
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
            abandonments: Cell::new(0),
            twin_refused: Cell::new(false),
            back_jumps: Cell::new(0),
            optimized: Cell::new(NO_BODY),
            optimized_version: Cell::new(0),
            optimized_traps: Cell::new(0),
            optimized_refused: Cell::new(false),
            copies: RefCell::new(Vec::new()),
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
            FIRST_ABANDONED..=LAST_ABANDONED => {
                write!(formatter, "JitSlot::Abandoned({})", self.exits.get())
            }
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
    /// Whether the run loop's push of the body has anything to do with the
    /// JIT out of line (`entered`): true while the body counts its entries
    /// or is compiled, and for the interpreted entry that revives an
    /// abandoned body; false once it is refused or exits at once. A push of
    /// a counting or compiled body pays one load and one compare
    /// (`vm::run_stretch`); a push of an abandoned body counts its entry
    /// here, inline, adding one to its state, so that only the push that
    /// revives it pays the call (`FIRST_ABANDONED`).
    #[inline(always)]
    pub(crate) fn push_is_armed(&self) -> bool {
        let state = self.state.get();
        if state <= COMPILED {
            return true;
        }
        if state < LAST_ABANDONED {
            self.state.set(state + 1);
            return false;
        }
        state == LAST_ABANDONED
    }

    /// Whether an interpreted backward jump in the body has anything to do
    /// with the JIT out of line (`back_edge_ready`): true while the body
    /// counts its entries or is compiled, so that its loop counts its back
    /// jumps or switches into its machine code; false once it is
    /// abandoned, refused or exits at once, which no back jump changes. The
    /// one load and compare the interpreter's backward jump pays inline for
    /// the JIT.
    #[inline(always)]
    pub(crate) fn back_edge_is_armed(&self) -> bool {
        self.state.get() <= COMPILED
    }

    /// Counts an interpreted backward jump towards `threshold`, answering
    /// whether this one reaches it, which starts the count afresh: after
    /// every try, so that a compile the exhausted zone holds, or a switch
    /// the frame could not make, is tried again only as many back jumps
    /// later, each held compile counting towards the next compaction
    /// (`JitState::hold_compile`).
    fn count_back_jump(&self, threshold: u32) -> bool {
        let back_jumps = self.back_jumps.get() + 1;
        if back_jumps < threshold {
            self.back_jumps.set(back_jumps);
            return false;
        }
        self.back_jumps.set(0);
        true
    }

    /// Whether the JIT abandoned the body and has not revived it yet.
    fn is_abandoned(&self) -> bool {
        (FIRST_ABANDONED..=LAST_ABANDONED).contains(&self.state.get())
    }

    /// The interpreted entries left before the abandoned body is revived,
    /// the one that revives it included.
    #[cfg(test)]
    fn entries_before_revival(&self) -> u32 {
        debug_assert!(self.is_abandoned());
        EXITS_AT_ONCE - self.state.get()
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

    /// Halves the count of the body's exits and of its abandonments
    /// (`JitState::decay_exits`), answering the exits: in a run that
    /// allocates, the wait of its next revival by entries shrinks again as
    /// its exits decay (`revival_entries`). Halves each copy's traps too,
    /// so that traps far apart in a long run do not add up to
    /// `TRAP_LIMIT`, as Strongtalk's sweeper decays its uncommon-trap
    /// counters; a copy's version never decays, or a copy that keeps
    /// failing would never reach its last.
    fn decay_exits(&self) -> u32 {
        let exits = self.exits.get() / 2;
        self.exits.set(exits);
        self.abandonments.set(self.abandonments.get() / 2);
        self.optimized_traps.set(self.optimized_traps.get() / 2);
        for copy in self.copies.borrow_mut().iter_mut() {
            copy.traps /= 2;
        }
        exits
    }

    /// The version of the copy for `key` -- the row's for a key, the
    /// slot's own for the uncustomized copy, `None` -- 0 for a key with
    /// no row yet.
    fn optimized_version(&self, key: Option<CustomizationKey>) -> u8 {
        match key {
            Some(key) => self.copy_row(key).map_or(0, |row| row.version),
            None => self.optimized_version.get(),
        }
    }

    /// Counts a trap of the copy for `key`, answering its traps now.
    fn count_trap(&self, key: Option<CustomizationKey>) -> u8 {
        let raise = |traps: u8| traps.saturating_add(1);
        match key {
            Some(key) => self.with_copy_row(key, |copy| {
                copy.traps = raise(copy.traps);
                copy.traps
            }),
            None => {
                let traps = raise(self.optimized_traps.get());
                self.optimized_traps.set(traps);
                traps
            }
        }
    }

    /// Raises the version of the copy for `key`, its traps counted from
    /// none at the new one, answering the version raised to; or, for a
    /// copy already past `LAST_VERSION`, which compiled without a trap and
    /// failed all the same, refuses the key for good and answers `None`.
    fn raise_version(&self, key: Option<CustomizationKey>) -> Option<u8> {
        let raise = |version: u8, traps: &mut u8, refused: &mut bool| {
            *traps = 0;
            if version > LAST_VERSION {
                *refused = true;
                return None;
            }
            Some(version + 1)
        };
        match key {
            Some(key) => self.with_copy_row(key, |copy| {
                let raised = raise(copy.version, &mut copy.traps, &mut copy.refused);
                copy.version = raised.unwrap_or(copy.version);
                raised
            }),
            None => {
                let (mut traps, mut refused) = (0, self.optimized_refused.get());
                let raised = raise(self.optimized_version.get(), &mut traps, &mut refused);
                self.optimized_traps.set(traps);
                self.optimized_refused.set(refused);
                if let Some(version) = raised {
                    self.optimized_version.set(version);
                }
                raised
            }
        }
    }

    /// Runs `change` on the row for `key`, which exists.
    fn with_copy_row<T>(
        &self,
        key: CustomizationKey,
        change: impl FnOnce(&mut CustomizedCopy) -> T,
    ) -> T {
        let mut copies = self.copies.borrow_mut();
        let Some(copy) = copies.iter_mut().find(|copy| copy.key == key) else {
            unreachable!("internal: a copy's row is made with its request");
        };
        change(copy)
    }

    fn settle(&self, state: u32) {
        self.state.set(state);
        self.zone.set(0);
        self.forget_optimized();
    }

    /// Forgets the body's optimized code, its one copy and every
    /// customized one, keeping each copy's row (`JitSlot::copies`).
    fn forget_optimized(&self) {
        self.optimized.set(NO_BODY);
        for copy in self.copies.borrow_mut().iter_mut() {
            copy.body = NO_BODY;
        }
    }

    /// Whether the body has a row for some key: only then is a key worth
    /// computing for an entry into it.
    pub(crate) fn has_copies(&self) -> bool {
        !self.copies.borrow().is_empty()
    }

    /// Whether some key's row names an installed copy: only then can the
    /// key of an entry change which code it enters, so that an entry into
    /// a body whose copies are all requested, freed or refused pays for
    /// no key, and an interpreted send into it enters its baseline code
    /// directly (`jit::called`).
    #[inline]
    fn has_installed_copy(&self) -> bool {
        self.copies.borrow().iter().any(|copy| copy.body != NO_BODY)
    }

    /// How many keys' rows name an installed copy, a freed one whose row
    /// still names it included: at most `DISPATCHING_ROWS` of them stand
    /// in the body's dispatching rows.
    #[inline]
    fn installed_copies(&self) -> usize {
        self.copies
            .borrow()
            .iter()
            .filter(|copy| copy.body != NO_BODY)
            .count()
    }

    /// The row for `key`, copied out.
    fn copy_row(&self, key: CustomizationKey) -> Option<CustomizedCopy> {
        self.copies
            .borrow()
            .iter()
            .find(|copy| copy.key == key)
            .copied()
    }

    /// Records the refusal of the copy for `key`, whose row exists.
    fn refuse_copy(&self, key: CustomizationKey) {
        self.with_copy_row(key, |copy| copy.refused = true);
    }

    /// Sets the body of the row for `key`, which exists; an installed
    /// one counts its traps from none, however its predecessor was freed.
    fn set_copy_body(&self, key: CustomizationKey, body: u32) {
        self.with_copy_row(key, |copy| {
            copy.body = body;
            if body != NO_BODY {
                copy.traps = 0;
            }
        });
    }

    /// Puts the body back to counting its entries, from none, its exits
    /// forgotten with its code: a compile that found the zone full, and a
    /// compaction that freed its machine code by its usage (`jit::zone`),
    /// and the revival of an abandoned body (`JitState::decay_exits`,
    /// `JitState::note_abandoned_entry`), leave it to be compiled again at
    /// its next `COMPILE_AT_ENTRY`th entry, as Cog forgets the machine code
    /// of a method it freed. Its abandonments are kept, so that the next
    /// revival waits the longer.
    fn forget(&self) {
        self.settle(0);
        self.exits.set(0);
        self.back_jumps.set(0);
    }

    /// Settles the body abandoned, counting the abandonment, its state
    /// the interpreted entries its revival waits for short of
    /// `EXITS_AT_ONCE` (`FIRST_ABANDONED`).
    fn abandon(&self, revival_base: u32) {
        let abandonments = self.abandonments.get().saturating_add(1);
        self.abandonments.set(abandonments);
        self.settle(EXITS_AT_ONCE - revival_entries(revival_base, abandonments));
    }

    /// Records the body compiled into `zone` at `offset`, its instruction
    /// at program counter 0 at `first_instruction`, the installed body
    /// `map`. Its exits count from none: the budget is that of this
    /// machine code, and a body compiled again into a replaced zone, whose
    /// old code `decay_exits` no longer reaches, does not carry the count
    /// the old code's exits left.
    fn compiled(&self, zone: u64, offsets: [CodeOffset; 2], map: u32) {
        let [offset, first_instruction] = offsets;
        self.exits.set(0);
        self.back_jumps.set(0);
        self.state.set(COMPILED);
        self.zone.set(zone);
        self.offset.set(offset.0);
        self.first_instruction.set(first_instruction.0);
        self.map.set(map);
        self.needed.set(0);
        self.forget_optimized();
    }

    /// Records the body's machine code moved by a compaction's slide
    /// (`jit::zone`) to `offset`, its instruction at program counter 0 to
    /// `first_instruction`, its installed body renumbered `map`, within
    /// the zone it was compiled into. Only where the code lies changes:
    /// the machine code is the same, so its exits keep counting towards
    /// the budget -- which only their decay halves -- and the body stays
    /// compiled, as a compaction may move it however often the zone fills.
    fn relocated(&self, offsets: [CodeOffset; 2], map: u32) {
        debug_assert_eq!(
            self.state.get(),
            COMPILED,
            "internal: a compaction moves only a body its slot names"
        );
        let [offset, first_instruction] = offsets;
        self.offset.set(offset.0);
        self.first_instruction.set(first_instruction.0);
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
    /// The shared tail of every method body's dispatching entry
    /// (`targets::assemble_dispatching_stub`), and the tail that counts
    /// the calls it enters a copy by, which a body compiled while the JIT
    /// counts jumps to.
    dispatching_stub: CodeOffset,
    counting_dispatching_stub: CodeOffset,
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
    /// The assembler could not lay the body out, for the reason it
    /// names; `None` in a build without a JIT (`cfg(ns_jit)` unset),
    /// where nothing is assembled.
    Unassembled(Option<ns_assembler::AssemblyError>),
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
    /// Whether the body is a twin, which reifies its frame and keeps the
    /// words its transfer and its templates wrote
    /// (`baseline::BodyCopy::Twin`, `MachineFrame::reifies`): whether a
    /// frame reifies is decided by the code it runs.
    reifies: bool,
    /// A lean body's twin, by its index in `bodies`, once a transfer or a
    /// resumption compiled it (`compile_twin`); `None` for a twin.
    twin: Option<u32>,
    /// A twin's lean body, by its index in `bodies`; `None` for a lean
    /// body. A twin is freed with its lean body, moves on its own and
    /// is named by no cell and no machine entry row.
    lean: Option<u32>,
    /// A lean body's transfers (`closures::transfer_to_twin`), each the program
    /// counter of a `MakeClosure` and the offset from the body's start of
    /// the data word holding the twin's address of it: 0 while the body
    /// has no twin.
    twin_transfers: Vec<TwinTransferWord>,
    /// The bytes a compile of the twin needed, its alignment's slack
    /// included, when it last found no room; 0 if none did, as
    /// `JitSlot::needed` is the lean body's.
    twin_needed: u32,
    /// The settings the body was compiled under.
    settings: CompileSettings,
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
    /// The offset from the body's start of the data word its spends of
    /// the optimization budget read the row's address from
    /// (`MacroAssembler::spend_budget`), which a compaction keeps; `None`
    /// for code that spends none.
    budget_word: Option<u32>,
    /// Whether the code is a body's baseline code -- a lean body or its
    /// twin -- or its optimized code.
    tier: Tier,
    /// Optimized code's baseline body, the lean one, by its index in
    /// `bodies`; `None` for baseline code. Optimized code has no twin of
    /// its own: its transfers go into its baseline body's twin. It is
    /// freed whenever its baseline body is, and alone only when its own
    /// exits abandon it (`abandon_optimized`), the baseline body staying;
    /// a compaction's freeing by usage judges the pair as one.
    baseline: Option<u32>,
    /// The offset from the body's start where the run loop's push enters
    /// it, past the prologue a compiled call enters at.
    push_entry: u32,
    /// How many times optimized code handed its frame to the interpreter
    /// (`EXIT_BUDGET`): its own count, so that its exits abandon it alone
    /// and never its baseline code, whose slot counts the baseline's
    /// (`count_exit`). Unused for baseline code.
    exits: Cell<u32>,
    /// The bytes a compile of the body's optimized code needed, its
    /// alignment's slack included, when it last found no room; 0 if none
    /// did, as `JitSlot::needed` is the lean body's own and `twin_needed`
    /// its twin's. Unused but for a lean body.
    optimized_needed: u32,
    /// The key a customized copy was compiled for, whose row in its
    /// slot's `copies` names it while it is current; `None` for baseline
    /// code and for a body's one uncustomized copy.
    key: Option<CustomizationKey>,
    /// A method's lean body's dispatching entry and its data words
    /// (`targets::emit_dispatching_entry`); `None` for every other code.
    dispatching: Option<InstalledDispatching>,
}

/// A method's lean body's dispatching entry as installed: the offsets
/// from the body's start of the entry and of its two data words, which a
/// compaction keeps, and the address of the dispatching rows the rows'
/// word names (`heap/optimized_copies.rs`): the vacant block's until the
/// JIT points it at the block of the body's code id.
#[derive(Clone, Copy, Debug)]
struct InstalledDispatching {
    entry: u32,
    rows_word: u32,
    tail_word: u32,
    rows: u64,
}

/// Which of the JIT's two compilers made a body's machine code
/// (`docs/design/execution.md`, "Planned: the optimizing JIT"). Optimized
/// code lives beside its body's baseline code, never in its place: a push,
/// a compiled call and a machine entry row enter it, while a frame that
/// goes on mid-body -- returned to, switched at a back edge, resumed after
/// a safepoint -- enters the baseline code, the only code with an entry at
/// every instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tier {
    Baseline,
    Optimized,
}

/// The settings of the JIT's state a body was compiled under, which its
/// twin is compiled under too, so that both copies are the same lowering
/// however a test changed the state between them
/// (`JitState::set_counting`, `JitState::set_futures_exit`).
#[derive(Clone, Copy, Debug)]
struct CompileSettings {
    counting: bool,
    futures_exit: bool,
}

/// A lean body's `MakeClosure` and its transfer word: the instruction's
/// program counter, and the word's offset from the body's start, which a
/// compaction keeps.
#[derive(Clone, Copy, Debug)]
struct TwinTransferWord {
    program_counter: u32,
    offset: u32,
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
    /// `None` when this build has no JIT (`cfg(ns_jit)` unset), the JIT is turned off by
    /// `NS_NEXT_NO_JIT=1`, or the zone could not be reserved.
    zone: Option<Zone>,
    /// Why the JIT is off though it was wanted: the zone could not be
    /// reserved, as the error says (`Interpreter::jit_unavailable`).
    /// `None` while it runs, and while it is off because it was not
    /// wanted.
    unavailable: Option<String>,
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
    /// The bodies the last lookups by an address inside the code found
    /// (`installed_index_at`), so that a leaf handed the code's own
    /// address rather than a call's return address finds its body
    /// without the search over `bodies` (`BodyLookup`).
    body_lookup: BodyLookup,
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
    /// The interpreted backward jump that compiles a body
    /// (`COMPILE_AT_BACK_JUMP`, but in a test that sets another).
    compile_at_back_jump: u32,
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
    /// Whether the innermost frame the last walk materialized stood in
    /// optimized code, whose exits count against its own budget, never
    /// its baseline code's (`count_exit`): the code of the exit's
    /// instruction, or, for a refused push, the code the caller's call
    /// returns into. Cleared with `walked` before every entry.
    pub(crate) walked_optimized: bool,
    /// Whether the miss leaf refused the last call for a callee that has
    /// no machine code yet but may still be compiled -- counting its
    /// entries, never compiled or freed by a compaction: a refusal that
    /// says nothing of the caller's code (`resumption_after_exit`).
    /// Cleared with `walked` before every entry.
    pub(crate) refused_for_an_uncompiled_callee: bool,
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
    /// How far the zone's next shrink goes (`shrink_zone`). It advances at
    /// every shrink the band allows, after a compaction and at the end of a
    /// full collection alike, so its ramp counts shrinks, not collections:
    /// a safepoint that serves both a compaction and a collection advances
    /// it twice, the second time with the growth flag the first took
    /// (`docs/design/memory.md`, "Giving committed memory back").
    zone_damping: crate::heap::limits::ShrinkDamping,
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
    /// Whether a body was revived by its interpreted entries
    /// (`note_abandoned_entry`) and the cells that refuse its calls are
    /// still to be written afresh (`take_revival`).
    revived_by_entries: bool,
    /// The interpreted entries a first abandonment waits before its
    /// revival: `REVIVAL_ENTRIES`, but in a test that lowers it
    /// (`set_revival_entries`).
    revival_base: u32,
    /// The bodies whose optimization budget ran out, each once per key,
    /// held weakly as the installed bodies are: what the optimizing
    /// compile at the next safepoint takes up.
    pub(crate) optimization_requests: Vec<OptimizationRequest>,
    /// The installed bodies, by index in `bodies`, whose budget word is
    /// still to be pointed at their row, which only a caller holding the
    /// heap can do (`after_compile_trigger`). Emptied by a compaction,
    /// which renumbers the bodies: a word left naming the unpointed
    /// budget (`MachineWord::UnpointedBudget`) is pointed at its first
    /// spend.
    budget_words_to_point: Vec<u32>,
    /// The budget a body's row is armed with: `OPTIMIZATION_BUDGET`, but
    /// in a test that sets another (`set_optimization_budget`), and 0
    /// under the optimize stress mode, whatever was set.
    optimization_budget: i64,
    /// The budget set for a body's row, which `optimization_budget` arms
    /// with but under the optimize stress mode.
    budget_set: i64,
    /// Whether the optimize stress mode is on (`NS_NEXT_STRESS_OPTIMIZE`,
    /// `set_optimize_stress`): every body's budget armed at 0, so that
    /// its first spend requests its optimization, and every body compiled
    /// to baseline code at its first entry and its first interpreted
    /// backward jump.
    optimize_stress: bool,
    /// Whether the deopt stress mode is on (`NS_NEXT_STRESS_DEOPT`,
    /// `set_deopt_stress`), which implies the optimize stress mode:
    /// optimized code compiled meanwhile counts its interrupt points down
    /// to a forced exit (`MachineWord::DeoptCountdown`).
    deopt_stress: bool,
    /// Whether the invalidate stress mode is on (`NS_NEXT_STRESS_INVALIDATE`,
    /// `set_invalidate_stress`), which implies the optimize stress mode:
    /// once optimized code has passed `STRESS_INVALIDATION_POINTS`
    /// interrupt points, it leaves for a safepoint, which frees every
    /// body's copies but those of the bodies backing off
    /// (`invalidate_optimized_for_stress`).
    invalidate_stress: bool,
    /// Each body's back-off under the invalidate stress mode, by its
    /// address, checked against the `Weak` it holds.
    #[cfg(ns_jit)]
    stress_back_offs: IdentityMap<usize, StressBackOff>,
    /// The method bodies that have a key's row (`JitSlot::copies`), held
    /// weakly as the installed bodies are: what an invalidation by
    /// application (`invalidate_applications`, `forget_freed_copies`)
    /// walks, since a row outlives its copy's code and its baseline code
    /// alike, and a row keyed on a reclaimed id must be dropped wherever
    /// it lies.
    #[cfg(ns_jit)]
    customized: Vec<Weak<CompiledCode>>,
    /// The state of the sequence that draws the next safepoint's
    /// countdown from `DEOPTIMIZATION_ROTATION` (`DEOPTIMIZATION_SEED`).
    deoptimization_draw: u64,
    /// The countdown the last safepoint set, which a forced exit came at
    /// (`JitStatistics::deepest_forced_exit`).
    deoptimization_countdown: u64,
    /// The machine words the unpointed budget lies in
    /// (`MachineWord::UnpointedBudget`), which every budget word names
    /// until it is pointed at its row: the heap's, shared when the world
    /// is made (`share_machine_words`), or a state's own in a test that
    /// has no heap.
    machine_words: crate::heap::machine_words::MachineWords,
    /// The dispatching rows of the method bodies' optimized copies, by
    /// code id (`heap/optimized_copies.rs`), reserved at the first install
    /// of a method body: what the bodies' dispatching entries compare a
    /// `super` or private lexical send's receiver and current class with.
    #[cfg(ns_jit)]
    optimized_copies: Option<crate::heap::optimized_copies::OptimizedCopies>,
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

/// Whether `NS_NEXT_STRESS_OPTIMIZE` asks for the optimize stress mode:
/// set, and neither empty nor `0`, or the deopt or the invalidate stress
/// mode, which imply it. Debug builds only; read once per process. Every
/// body is then optimized as early as it can be: compiled to baseline code
/// at its first entry or interpreted backward jump, its budget armed at 0,
/// so that its first spend requests its optimization, which the next
/// safepoint compiles -- V8's `stress_maglev` (`docs/design/execution.md`,
/// "The optimizing tier's foundation", "Verification"). It forces the JIT
/// on where it is available.
pub(crate) fn stress_optimize_from_env() -> bool {
    static REQUESTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    stress_deopt_from_env()
        || stress_invalidate_from_env()
        || (cfg!(debug_assertions)
            && *REQUESTED.get_or_init(|| {
                std::env::var_os("NS_NEXT_STRESS_OPTIMIZE")
                    .is_some_and(|value| !value.is_empty() && value != "0")
            }))
}

/// Whether `NS_NEXT_STRESS_DEOPT` asks for the deopt stress mode: set, and
/// neither empty nor `0`. Debug builds only; read once per process. It
/// implies the optimize stress mode, and optimized code leaves at every
/// n-th interrupt point it passes, n drawn from
/// `DEOPTIMIZATION_ROTATION` afresh at every safepoint, so that every
/// point is met -- V8's `deopt_every_n_times`, HotSpot's `DeoptimizeALot`
/// (`docs/design/execution.md`, "The optimizing tier's foundation",
/// "Verification").
pub(crate) fn stress_deopt_from_env() -> bool {
    static REQUESTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    cfg!(debug_assertions)
        && *REQUESTED.get_or_init(|| {
            std::env::var_os("NS_NEXT_STRESS_DEOPT")
                .is_some_and(|value| !value.is_empty() && value != "0")
        })
}

/// Whether `NS_NEXT_STRESS_INVALIDATE` asks for the invalidate stress
/// mode: set, and neither empty nor `0`. Debug builds only; read once per
/// process. It implies the optimize stress mode, and optimized code counts
/// its interrupt points: at the `STRESS_INVALIDATION_POINTS`th since the
/// last round it leaves for a safepoint, which frees the copies of every
/// body but those backing off (`invalidate_optimized_for_stress`) before
/// it installs what was requested since, so that every copy freed has had
/// a stretch to run, and its body is optimized afresh at its next run-out
/// (`docs/design/execution.md`, "Speculation", "Verification").
pub(crate) fn stress_invalidate_from_env() -> bool {
    static REQUESTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    cfg!(debug_assertions)
        && *REQUESTED.get_or_init(|| {
            std::env::var_os("NS_NEXT_STRESS_INVALIDATE")
                .is_some_and(|value| !value.is_empty() && value != "0")
        })
}

/// The interrupt points optimized code passes between two rounds of the
/// invalidate stress mode (`JitState::arm_invalidation_count`): enough
/// that the copies installed since the last round run past their entry
/// before they are freed, few enough that a program whose optimized code
/// runs at all -- a golden's few sends -- meets a round, as a hundred did
/// not for most goldens. A body freed often backs off on its own
/// (`StressBackOff`).
const STRESS_INVALIDATION_POINTS: u64 = 10;

/// The most rounds of the invalidate stress mode one body sits out
/// between two invalidations of its copies, as a power of two
/// (`StressBackOff`): a body freed at a round sits out the next one, then
/// two, then four, up to 2 to this power, as an abandoned body's revival
/// backs off (`revival_entries`), so that a body every round would
/// recompile -- a hundred-send body takes a debug build seconds to
/// optimize -- is recompiled a number of times that grows with the
/// logarithm of the run's length until the cap, and every body is still
/// invalidated now and then in a long run.
const MAXIMUM_STRESS_BACK_OFF_DOUBLINGS: u32 = 10;

/// One body's back-off under the invalidate stress mode
/// (`MAXIMUM_STRESS_BACK_OFF_DOUBLINGS`): how many times a round freed its
/// copies, and how many rounds it has sat out since the last.
#[cfg(ns_jit)]
struct StressBackOff {
    code: Weak<CompiledCode>,
    invalidations: u32,
    rounds_sat_out: u32,
}

#[cfg(ns_jit)]
impl StressBackOff {
    /// Whether this round frees the body's copies, counting the round
    /// either way: the first round always does, and each after waits
    /// twice as many rounds as the last, up to the cap.
    fn take_round(&mut self) -> bool {
        let doublings = self
            .invalidations
            .saturating_sub(1)
            .min(MAXIMUM_STRESS_BACK_OFF_DOUBLINGS);
        let wait = if self.invalidations == 0 {
            0
        } else {
            1_u32 << doublings
        };
        if self.rounds_sat_out < wait {
            self.rounds_sat_out += 1;
            return false;
        }
        self.invalidations = self.invalidations.saturating_add(1);
        self.rounds_sat_out = 0;
        true
    }
}

/// The countdowns the deopt stress mode's safepoints draw from
/// (`MachineWord::DeoptCountdown`), the Fibonacci numbers up to 987:
/// small ones, so that the exit falls on every point of a short stretch
/// of optimized code, and large ones, so that it falls deep in a long
/// one too. A forced exit leaves its activation's optimized code for
/// good, since a frame goes back into it only at a push, and holds the
/// safepoint that sets the next countdown, so that an activation's n-th
/// point is forced only by a countdown of at least n: a rotation that
/// stopped at 13 would never force an exit past a loop's first few turns,
/// where the values its back edge carries are written back.
const DEOPTIMIZATION_ROTATION: [u64; 15] =
    [1, 2, 3, 5, 8, 13, 21, 34, 55, 89, 144, 233, 377, 610, 987];

/// The seed of the xorshift sequence that draws each safepoint's
/// countdown from `DEOPTIMIZATION_ROTATION`
/// (`JitState::rotate_deoptimization_countdown`), set afresh whenever the
/// mode is turned on, so that a run's forced exits reproduce. Drawn, not
/// walked in order: an order of fixed period locks in step with any
/// activation that spans a multiple of its safepoints, which then starts
/// at the same countdown every time and is never forced at the points
/// the others name.
const DEOPTIMIZATION_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

impl JitState {
    /// The state of a new interpreter: a zone of `DEFAULT_ZONE_BYTES`,
    /// unless `NS_NEXT_NO_JIT=1` turns the JIT off.
    pub(crate) fn new() -> JitState {
        let stress_jit = stress_jit_from_env();
        let optimize_stress = stress_optimize_from_env();
        let stress =
            stress_jit || stress_exit_from_env() || stress_zone_from_env() || optimize_stress;
        let wanted = available()
            && (stress || std::env::var_os("NS_NEXT_NO_JIT").is_none_or(|value| value != "1"));
        let (zone, unavailable) = if wanted {
            match reserve_zone(DEFAULT_ZONE_BYTES) {
                Ok(zone) => (Some(zone), None),
                Err(reason) => (None, Some(reason)),
            }
        } else {
            (None, None)
        };
        JitState {
            enabled: zone.is_some(),
            zone,
            unavailable,
            bodies: Vec::new(),
            returns: IdentityMap::default(),
            body_lookup: BodyLookup::default(),
            counting: false,
            statistics: JitStatistics::default(),
            install_bodies_exiting_at_once: stress_jit,
            futures_exit: false,
            compile_at_entry: if stress_jit || optimize_stress {
                1
            } else {
                COMPILE_AT_ENTRY
            },
            compile_at_back_jump: if optimize_stress {
                1
            } else {
                COMPILE_AT_BACK_JUMP
            },
            #[cfg(ns_jit)]
            walk: materialize::Walk::default(),
            #[cfg(ns_jit)]
            cut: cut::CutWalk::default(),
            walked: None,
            walked_optimized: false,
            refused_for_an_uncompiled_callee: false,
            block: EntryBlock::new(),
            block_stale: true,
            compaction_requested: false,
            zone_exhausted: false,
            wanted_bytes: 0,
            held_compiles: 0,
            fruitless_compactions: 0,
            bytes_freed_since_compaction: 0,
            zone_damping: crate::heap::limits::ShrinkDamping::default(),
            #[cfg(debug_assertions)]
            entries_in_progress: 0,
            abandoned: Vec::new(),
            revived_by_entries: false,
            revival_base: REVIVAL_ENTRIES,
            optimization_requests: Vec::new(),
            budget_words_to_point: Vec::new(),
            optimization_budget: if optimize_stress {
                0
            } else {
                OPTIMIZATION_BUDGET
            },
            budget_set: OPTIMIZATION_BUDGET,
            optimize_stress,
            deopt_stress: stress_deopt_from_env(),
            invalidate_stress: stress_invalidate_from_env(),
            #[cfg(ns_jit)]
            stress_back_offs: IdentityMap::default(),
            #[cfg(ns_jit)]
            customized: Vec::new(),
            deoptimization_draw: DEOPTIMIZATION_SEED,
            deoptimization_countdown: 0,
            machine_words: crate::heap::machine_words::MachineWords::new(),
            #[cfg(ns_jit)]
            optimized_copies: None,
        }
    }

    /// Reserves a zone of `bytes`, recording why the JIT is off when none
    /// can be reserved.
    fn reserve(&mut self, bytes: usize) {
        match reserve_zone(bytes) {
            Ok(zone) => {
                self.zone = Some(zone);
                self.unavailable = None;
            }
            Err(reason) => {
                self.zone = None;
                self.unavailable = Some(reason);
            }
        }
    }

    /// Why the JIT is off though it was wanted (`Interpreter::jit_unavailable`).
    pub(crate) fn unavailable(&self) -> Option<&str> {
        self.unavailable.as_deref()
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

    /// The bytes of the code zone committed (`Interpreter::table_sizes`).
    pub(crate) fn zone_committed_bytes(&self) -> usize {
        self.zone.as_ref().map_or(0, |zone| zone.writer.committed())
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
            self.reserve(DEFAULT_ZONE_BYTES);
            self.mark_block_stale();
        }
        self.enabled = enabled && self.zone.is_some();
        // A JIT switched off is not wanted: no reason is reported for it.
        if !enabled {
            self.unavailable = None;
            self.refuse_abandoned();
        }
        reserved
    }

    /// Refuses every body still abandoned, as the JIT switched off refuses
    /// a body at its first entry: an abandoned body's push counts its
    /// entries inline and reaches `entered` only at the one that would
    /// revive it (`JitSlot::push_is_armed`), so without this its pushes
    /// would go on paying the count while the JIT is off, and it would be
    /// revived once the JIT is on again. Its machine code was freed and its
    /// cells name the refusing stub since its abandonment, so nothing else
    /// names it.
    fn refuse_abandoned(&mut self) {
        for body in std::mem::take(&mut self.abandoned) {
            let Some(code) = body.upgrade() else {
                continue;
            };
            if code.jit.is_abandoned() {
                code.jit.refuse();
                self.statistics.refused_bodies += 1;
            }
        }
    }

    /// Replaces the zone with one that installs code in at most `bytes`.
    /// Every body compiled into the old zone is compiled again into the
    /// new one at its next entry, since its slot names the old zone. A
    /// cell that names an address in the old zone must be vacated with it
    /// (`Interpreter::set_jit_zone_bytes` invalidates the caches).
    ///
    /// The abandoned bodies stay abandoned: none has machine code in
    /// either zone, and the revival compiles it into the new one, so the
    /// replacement leaves their waits as they are. Only a replacement that
    /// leaves the JIT off -- the new zone could not be reserved -- refuses
    /// them, as switching the JIT off does (`refuse_abandoned`), so that a
    /// JIT off never revives one.
    pub(crate) fn replace_zone(&mut self, bytes: usize) {
        self.bodies.clear();
        // Every row's entry lies in the old zone.
        #[cfg(ns_jit)]
        if let Some(table) = self.optimized_copies.as_mut() {
            table.clear_all();
        }
        self.returns.clear();
        self.budget_words_to_point.clear();
        self.compaction_requested = false;
        self.zone_exhausted = false;
        self.wanted_bytes = 0;
        self.held_compiles = 0;
        self.fruitless_compactions = 0;
        self.bytes_freed_since_compaction = 0;
        self.zone = None;
        if available() {
            self.reserve(bytes);
        }
        self.enabled = self.enabled && self.zone.is_some();
        if !self.enabled {
            self.refuse_abandoned();
        }
        self.mark_block_stale();
    }

    /// Makes the unpointed budget the one among `words`, the heap's, so
    /// that the JIT's every word machine code spends from lies in the
    /// heap's machine words or its budget table. Before any body is
    /// compiled.
    pub(crate) fn share_machine_words(&mut self, words: crate::heap::machine_words::MachineWords) {
        debug_assert!(
            self.bodies.is_empty(),
            "internal: the machine words are shared before any compile"
        );
        self.machine_words = words;
        self.rotate_deoptimization_countdown();
        self.arm_invalidation_count();
    }

    /// The unpointed budget's address (`MachineWord::UnpointedBudget`).
    #[cfg(ns_jit)]
    fn unpointed_budget_address(&self) -> u64 {
        use crate::heap::machine_words::{MachineWord, MachineWords};
        let offset = MachineWords::offset(MachineWord::UnpointedBudget).unsigned_abs();
        self.machine_words.address() as u64 + u64::from(offset)
    }

    /// Sets the budget every body's row is armed with from now on
    /// (`OPTIMIZATION_BUDGET` otherwise) -- but 0 while the optimize
    /// stress mode is on -- and answers the budget armed.
    pub(crate) fn set_optimization_budget(&mut self, budget: i64) -> i64 {
        self.budget_set = budget;
        self.optimization_budget = if self.optimize_stress { 0 } else { budget };
        self.optimization_budget
    }

    /// Turns the optimize stress mode on or off for this state, as
    /// `NS_NEXT_STRESS_OPTIMIZE` does at its creation, answering the
    /// budget every row is armed with from now on; turning it off turns
    /// the deopt and the invalidate stress modes off too, which imply it.
    /// Set before the code it is meant for runs.
    pub(crate) fn set_optimize_stress(&mut self, stress: bool) -> i64 {
        self.optimize_stress = stress;
        self.deopt_stress = self.deopt_stress && stress;
        self.invalidate_stress = self.invalidate_stress && stress;
        self.compile_at_entry = if stress || self.install_bodies_exiting_at_once {
            1
        } else {
            COMPILE_AT_ENTRY
        };
        self.compile_at_back_jump = if stress { 1 } else { COMPILE_AT_BACK_JUMP };
        self.set_optimization_budget(self.budget_set)
    }

    /// Turns the deopt stress mode on or off for this state, as
    /// `NS_NEXT_STRESS_DEOPT` does at its creation: on, with the optimize
    /// stress mode and the countdown drawn from its seed; off, the
    /// optimize stress mode left as it is. Optimized code compiled before
    /// keeps its points uncounted. Answers the budget every row is armed
    /// with from now on.
    pub(crate) fn set_deopt_stress(&mut self, stress: bool) -> i64 {
        let budget = if stress {
            self.set_optimize_stress(true)
        } else {
            self.optimization_budget
        };
        self.deopt_stress = stress;
        self.deoptimization_draw = DEOPTIMIZATION_SEED;
        self.rotate_deoptimization_countdown();
        self.arm_invalidation_count();
        budget
    }

    /// Turns the invalidate stress mode on or off for this state, as
    /// `NS_NEXT_STRESS_INVALIDATE` does at its creation: on, with the
    /// optimize stress mode; off, the optimize stress mode left as it is.
    /// Answers the budget every row is armed with from now on.
    pub(crate) fn set_invalidate_stress(&mut self, stress: bool) -> i64 {
        let budget = if stress {
            self.set_optimize_stress(true)
        } else {
            self.optimization_budget
        };
        self.invalidate_stress = stress;
        self.arm_invalidation_count();
        budget
    }

    /// Whether the invalidate stress mode is on, under which optimized code
    /// leaves for a round of invalidation at every
    /// `STRESS_INVALIDATION_POINTS`th interrupt point it passes.
    pub(crate) fn invalidate_stress(&self) -> bool {
        self.invalidate_stress
    }

    /// Whether the invalidate stress mode's safepoint runs a round now:
    /// once the code compiled under the mode has spent the countdown the
    /// last round armed (`arm_invalidation_count`), whose last point left
    /// for this safepoint (`take_forced_exit`) -- or, under the deopt
    /// stress mode too, which sets the countdown at every safepoint, once
    /// it passed any point since the last. So every copy freed has had the
    /// chance to run: a round at every safepoint a run-out requests would
    /// free each copy before a frame entered it, and every body would ask
    /// for its copy again at its next spend, compiling at every safepoint
    /// for ever.
    pub(crate) fn invalidation_due_for_stress(&self) -> bool {
        use crate::heap::machine_words::MachineWord;
        let countdown = self.machine_words.get(MachineWord::DeoptCountdown);
        if self.deopt_stress {
            return countdown != self.deoptimization_countdown;
        }
        // Spent: at zero, or past it -- `take_forced_exit` sets it to
        // `u64::MAX`, below which every later point counts.
        countdown == 0 || countdown > self.deoptimization_countdown
    }

    /// Under the invalidate stress mode alone, arms the countdown at
    /// `STRESS_INVALIDATION_POINTS`, so that the point that spends it
    /// leaves for the next round (`take_forced_exit`): at the mode's start
    /// and after each round. Under the deopt stress mode too, its
    /// safepoints set the countdown themselves
    /// (`rotate_deoptimization_countdown`), and every one that follows a
    /// point passed runs a round.
    pub(crate) fn arm_invalidation_count(&mut self) {
        use crate::heap::machine_words::MachineWord;
        if self.invalidate_stress && !self.deopt_stress {
            self.deoptimization_countdown = STRESS_INVALIDATION_POINTS;
            self.machine_words
                .set(MachineWord::DeoptCountdown, STRESS_INVALIDATION_POINTS);
        }
    }

    /// Whether the invalidate stress mode's round frees `code`'s copies,
    /// counting the round in its back-off (`StressBackOff`).
    #[cfg(ns_jit)]
    fn stress_round_frees(&mut self, code: &Rc<CompiledCode>) -> bool {
        let key = Rc::as_ptr(code) as usize;
        let back_off = self
            .stress_back_offs
            .entry(key)
            .or_insert_with(|| StressBackOff {
                code: Rc::downgrade(code),
                invalidations: 0,
                rounds_sat_out: 0,
            });
        // Another body at the address of a dead one starts afresh.
        if !std::ptr::eq(back_off.code.as_ptr(), Rc::as_ptr(code))
            || back_off.code.strong_count() == 0
        {
            *back_off = StressBackOff {
                code: Rc::downgrade(code),
                invalidations: 0,
                rounds_sat_out: 0,
            };
        }
        back_off.take_round()
    }

    /// The live method bodies that have a key's row, the dead ones
    /// forgotten (`JitState::customized`).
    #[cfg(ns_jit)]
    fn customized_codes(&mut self) -> Vec<Rc<CompiledCode>> {
        self.customized.retain(|code| code.strong_count() > 0);
        self.customized.iter().filter_map(Weak::upgrade).collect()
    }

    /// Whether the optimize stress mode is on, under which a body's budget
    /// is never raised for its feedback to settle: it runs out at once.
    pub(crate) fn optimize_stress(&self) -> bool {
        self.optimize_stress
    }

    /// Under the deopt stress mode, sets the countdown to the next value
    /// drawn from `DEOPTIMIZATION_ROTATION` by a xorshift sequence from
    /// `DEOPTIMIZATION_SEED`, whose period of 2^64 - 1 draws every value
    /// again and again: what the run loop's safepoint does, so that of
    /// the points passed until the next safepoint the one the countdown
    /// names leaves. Nothing outside the mode.
    pub(crate) fn rotate_deoptimization_countdown(&mut self) {
        use crate::heap::machine_words::MachineWord;
        if !self.deopt_stress {
            return;
        }
        let mut draw = self.deoptimization_draw;
        draw ^= draw << 13;
        draw ^= draw >> 7;
        draw ^= draw << 17;
        self.deoptimization_draw = draw;
        let index = usize::try_from(draw % DEOPTIMIZATION_ROTATION.len() as u64)
            .expect("internal: an index into the rotation fits a usize");
        let countdown = DEOPTIMIZATION_ROTATION[index];
        self.deoptimization_countdown = countdown;
        self.machine_words
            .set(MachineWord::DeoptCountdown, countdown);
    }

    /// Whether the exit just taken was the deopt or the invalidate stress
    /// mode's own: the countdown at zero, which only the point that
    /// brought it there, and exited at once, leaves it at, since the
    /// safepoint sets it afresh to at least 1. The countdown is then spent
    /// past zero, so that no later exit is taken for one, and the
    /// safepoint request held, so that the stretch the exit goes on in
    /// leaves for the safepoint that sets the next countdown -- under the
    /// invalidate stress mode, the one that runs its round
    /// (`invalidation_due_for_stress`).
    #[cfg(ns_jit)]
    fn take_forced_exit(&mut self, heap: &mut crate::heap::Heap) -> bool {
        use crate::heap::machine_words::MachineWord;
        let counting = self.deopt_stress || self.invalidate_stress;
        if !counting || self.machine_words.get(MachineWord::DeoptCountdown) != 0 {
            return false;
        }
        self.machine_words
            .set(MachineWord::DeoptCountdown, u64::MAX);
        heap.hold_safepoint_request();
        self.statistics.forced_exits += 1;
        self.statistics.deepest_forced_exit = self
            .statistics
            .deepest_forced_exit
            .max(self.deoptimization_countdown);
        true
    }

    /// Turns the JIT stress mode on or off for this state, as
    /// `NS_NEXT_STRESS_JIT` does at its creation: every body compiled at
    /// its first entry, and one whose machine code exits at once
    /// installed and entered all the same. Set before the code it is meant
    /// for runs.
    pub(crate) fn set_stress(&mut self, stress: bool) {
        self.install_bodies_exiting_at_once = stress;
        self.compile_at_entry = if stress || self.optimize_stress {
            1
        } else {
            COMPILE_AT_ENTRY
        };
    }

    /// Makes `MakeFuture` lower to an exit in the bodies compiled from now
    /// on (`true`), as no body's does otherwise, so that a body making a
    /// future at once exits at once: what a test of a body never compiled
    /// needs, since every other instruction such a body could start with
    /// runs in machine code (`Interpreter::set_jit_futures_exit`).
    pub(crate) fn set_futures_exit(&mut self, futures_exit: bool) {
        self.futures_exit = futures_exit;
    }

    /// Sets the interpreted backward jump that compiles a body
    /// (`COMPILE_AT_BACK_JUMP` otherwise), at least the first.
    pub(crate) fn set_compile_at_back_jump(&mut self, jumps: u32) {
        self.compile_at_back_jump = jumps.max(1);
    }

    /// Counts a stretch of the run loop that ended at a backward jump to
    /// switch into machine code (`JitStatistics::back_edge_stretch_exits`).
    pub(crate) fn note_back_edge_stretch_exit(&mut self) {
        self.statistics.back_edge_stretch_exits += 1;
    }

    /// Counts a frame that goes back into machine code at its loop's head
    /// once the safepoint its backward jump stopped at is served
    /// (`JitStatistics::safepoint_resumptions`).
    pub(crate) fn note_safepoint_resumption(&mut self) {
        self.statistics.safepoint_resumptions += 1;
    }

    /// Makes the bodies compiled from now on count their back edges, and
    /// the counts reach the statistics while it is on.
    pub(crate) fn set_counting(&mut self, counting: bool) {
        self.counting = counting;
    }

    /// Sets the interpreted entries a first abandonment waits before its
    /// revival (`revival_entries`), `REVIVAL_ENTRIES` otherwise, so that a
    /// test reaches a revival by entries without thousands of them; at
    /// least one, and never more than `REVIVAL_ENTRIES`.
    pub(crate) fn set_revival_entries(&mut self, entries: u32) {
        self.revival_base = entries.clamp(1, REVIVAL_ENTRIES);
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
    /// one past its start -- the body, while it lives, and whether it is
    /// freed: a freed body's returns are gone, though its code may live
    /// on, as a slide of the site ranges frees the code of a live body
    /// whose base moved.
    #[cfg(all(test, debug_assertions))]
    pub(crate) fn installed_bodies_for_test(&self) -> Vec<(usize, Option<Rc<CompiledCode>>, bool)> {
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
                (start + 1, body.code.upgrade(), body.free)
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
    ///
    /// Inline into every caller, `upgraded` with it: answered through
    /// memory, the answer's narrow fields -- the feedback base, `reifies`,
    /// `optimized` -- are stored one by one and read back as one word, a
    /// load no store forwards, which stalled `materialize::leaf_frame` on
    /// every leaf's way into a frame machine code pushed.
    #[inline(always)]
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

    /// Writes `address` into the budget word of the body at `index` in
    /// `bodies`, through the zone's one writer, as every data word is
    /// written.
    #[cfg(ns_jit)]
    fn write_budget_word(&mut self, index: u32, address: u64) {
        let Some(body) = self.live_body(index) else {
            return;
        };
        let Some(word) = body.budget_word else {
            return;
        };
        let offset = body.start + word;
        let Some(zone) = self.zone.as_mut() else {
            unreachable!("internal: an installed body lies in a zone");
        };
        if zone
            .writer
            .write(CodeOffset(offset), &address.to_le_bytes())
            .is_err()
        {
            unreachable!("internal: an installed body's words lie in the committed zone");
        }
    }

    /// Writes the transfer words of the body at `index` in `bodies` -- a
    /// lean body or optimized code, whose transfers go into its baseline
    /// body's twin -- through the zone's one writer: each the address of
    /// its `MakeClosure`'s code in the twin, or 0 while the body has none,
    /// so that the next transfer compiles it (`closures::transfer_to_twin`).
    /// Written when the twin is compiled, when optimized code is installed
    /// beside a body that has one, and after a compaction moved either
    /// copy, between entries or from the twin leaf, before the transfer
    /// that called it reads its word again.
    pub(super) fn write_transfer_words(&mut self, index: u32) {
        let Some(zone) = self.zone.as_mut() else {
            return;
        };
        let body = &self.bodies[index as usize];
        let owner = body
            .baseline
            .map_or(body, |baseline| &self.bodies[baseline as usize]);
        let twin = owner.twin.map(|twin| &self.bodies[twin as usize]);
        for transfer in &body.twin_transfers {
            let address = twin.map_or(0, |twin| {
                let Some(offset) = twin.map.machine_offset(transfer.program_counter) else {
                    unreachable!("internal: a twin has code for every instruction");
                };
                zone.writer
                    .zone()
                    .executable_address(CodeOffset(twin.start + offset)) as u64
            });
            if zone
                .writer
                .write(
                    CodeOffset(body.start + transfer.offset),
                    &address.to_le_bytes(),
                )
                .is_err()
            {
                unreachable!("internal: an installed body's words lie in the committed zone");
            }
        }
    }

    /// Writes the transfer words of the lean body at `lean` and of the
    /// optimized code beside it, if it has some (`write_transfer_words`).
    pub(super) fn write_pair_transfer_words(&mut self, lean: u32) {
        self.write_transfer_words(lean);
        for optimized in self.optimized_copies_of(lean) {
            self.write_transfer_words(optimized);
        }
    }

    /// The indices in `bodies` of the optimized code beside the lean body
    /// at `lean` -- its one copy, or a method's customized copies -- while
    /// its slot names them and it.
    pub(super) fn optimized_copies_of(&self, lean: u32) -> Vec<u32> {
        let Some(code) = self
            .bodies
            .get(lean as usize)
            .and_then(|body| body.code.upgrade())
        else {
            return Vec::new();
        };
        if self.named_body(&code) != Some(lean as usize) {
            return Vec::new();
        }
        let live = |index: &u32| *index != NO_BODY && self.live_body(*index).is_some();
        let copies = code.jit.copies.borrow();
        std::iter::once(code.jit.optimized.get())
            .chain(copies.iter().map(|copy| copy.body))
            .filter(live)
            .collect()
    }

    /// The table of dispatching rows, reserved at its first use.
    #[cfg(ns_jit)]
    fn copies_table(&mut self) -> &mut crate::heap::optimized_copies::OptimizedCopies {
        self.optimized_copies
            .get_or_insert_with(crate::heap::optimized_copies::OptimizedCopies::new)
    }

    /// The bytes of the dispatching rows' reservation committed, 0 before
    /// its first use (`Interpreter::table_sizes`).
    pub(crate) fn optimized_copies_committed_bytes(&self) -> usize {
        #[cfg(ns_jit)]
        {
            self.optimized_copies.as_ref().map_or(
                0,
                crate::heap::optimized_copies::OptimizedCopies::committed_bytes,
            )
        }
        #[cfg(not(ns_jit))]
        0
    }

    /// Gives back the dispatching rows' committed pages above the blocks
    /// of `code_ids` ids, the code table's length after a full collection,
    /// as `shrink` and `damped` say (`OptimizedCopies::shrink`), answering
    /// the bytes given back. Called at the end of the collection, after
    /// every body's word was pointed afresh
    /// (`point_budget_words_after_collection`), so that none names a
    /// block given back, which a debug build checks.
    pub(crate) fn shrink_optimized_copies(
        &mut self,
        code_ids: usize,
        shrink: crate::heap::limits::Shrink,
        damped: bool,
    ) -> usize {
        #[cfg(ns_jit)]
        {
            let Some(table) = self.optimized_copies.as_mut() else {
                return 0;
            };
            let bytes = table.shrink(code_ids, shrink, damped);
            #[cfg(debug_assertions)]
            for body in self.bodies.iter().filter(|body| !body.free) {
                if let Some(dispatching) = body.dispatching {
                    assert!(
                        table.is_committed(dispatching.rows),
                        "internal: a dispatching word names a block given back"
                    );
                }
            }
            bytes
        }
        #[cfg(not(ns_jit))]
        {
            let _ = (code_ids, shrink, damped);
            0
        }
    }

    /// Writes the dispatching entry's data words of the lean body at
    /// `index` that are given -- the address of its rows, the tail's --
    /// through the zone's one writer, as every data word is written.
    #[cfg(ns_jit)]
    fn write_dispatching_words(&mut self, index: u32, rows: Option<u64>, tail: Option<u64>) {
        let Some(dispatching) = self.live_body(index).and_then(|body| body.dispatching) else {
            return;
        };
        let start = self.bodies[index as usize].start;
        if let Some(rows) = rows {
            self.bodies[index as usize].dispatching = Some(InstalledDispatching {
                rows,
                ..dispatching
            });
        }
        let Some(zone) = self.zone.as_mut() else {
            unreachable!("internal: an installed body lies in a zone");
        };
        for (offset, word) in [(dispatching.rows_word, rows), (dispatching.tail_word, tail)] {
            let Some(word) = word else {
                continue;
            };
            if zone
                .writer
                .write(CodeOffset(start + offset), &word.to_le_bytes())
                .is_err()
            {
                unreachable!("internal: an installed body's words lie in the committed zone");
            }
        }
    }

    /// Points the dispatching rows' word of the body at `index`, if it is
    /// a method's lean body, at the block of `id`, the code id its body
    /// has now -- or, given none, at the vacant block -- and writes the
    /// block from the body's copies (`write_dispatching_rows`): with its
    /// budget word, at install and afresh after every full collection,
    /// which may have freed the id the block was the block of and minted
    /// it for another body. Only the lean body its slot names takes the
    /// block, so that no other body's word writes a block the named one
    /// owns.
    #[cfg(ns_jit)]
    fn point_dispatching_rows(&mut self, index: u32, id: Option<usize>) {
        let Some(body) = self.live_body(index) else {
            return;
        };
        if body.dispatching.is_none() {
            return;
        }
        let named = body
            .code
            .upgrade()
            .is_some_and(|code| self.named_body(&code) == Some(index as usize));
        let rows = match id.filter(|_| named) {
            Some(id) => self.copies_table().block_address(id),
            None => self.copies_table().vacant_address(),
        };
        self.write_dispatching_words(index, Some(rows), None);
        self.write_dispatching_rows(index);
    }

    /// Writes the dispatching rows of the lean body at `lean` from the
    /// optimized copies its slot names beside it, at most
    /// `DISPATCHING_ROWS` of them, in the block its word names: the one
    /// way a block is written, called wherever a copy is installed or
    /// freed and after a compaction moved the copies. Nothing is written
    /// for a body its slot does not name, nor into the vacant block.
    #[cfg(ns_jit)]
    fn write_dispatching_rows(&mut self, lean: u32) {
        let Some(body) = self.live_body(lean) else {
            return;
        };
        let Some(address) = body.dispatching.map(|dispatching| dispatching.rows) else {
            return;
        };
        // A body its slot no longer names may still name the block of the
        // body that took its place, whose rows are that body's to write.
        let named = body
            .code
            .upgrade()
            .is_some_and(|code| self.named_body(&code) == Some(lean as usize));
        if !named {
            return;
        }
        let rows = self.dispatching_rows_of(lean);
        self.copies_table().write(address, &rows);
    }

    /// What the dispatching rows of the lean body at `lean` should hold:
    /// for each customized copy its slot names beside it, in order, the
    /// receiver's dispatch id and entered application word of its key and
    /// its machine entry.
    #[cfg(ns_jit)]
    fn dispatching_rows_of(&self, lean: u32) -> Vec<crate::heap::optimized_copies::Row> {
        let Some(zone) = self.zone.as_ref() else {
            return Vec::new();
        };
        let Some(code) = self
            .bodies
            .get(lean as usize)
            .and_then(|body| body.code.upgrade())
        else {
            return Vec::new();
        };
        self.optimized_copies_of(lean)
            .into_iter()
            .filter_map(|index| {
                let body = &self.bodies[index as usize];
                let row = code.jit.copy_row(body.key?)?;
                Some([
                    u64::from(row.key.receiver.bits()),
                    row.entered_word,
                    zone.writer
                        .zone()
                        .executable_address(CodeOffset(body.start)) as u64,
                ])
            })
            .take(crate::heap::optimized_copies::DISPATCHING_ROWS)
            .collect()
    }

    /// The address of the dispatching entry of `code`'s lean body, when
    /// it has machine code here and is a method's.
    #[cfg(ns_jit)]
    fn dispatching_entry_address(&self, code: &CompiledCode) -> Option<u64> {
        let index = self.named_body(code)?;
        let body = &self.bodies[index];
        let entry = body.dispatching?.entry;
        let zone = self.zone.as_ref()?;
        Some(
            zone.writer
                .zone()
                .executable_address(CodeOffset(body.start + entry)) as u64,
        )
    }

    /// The innermost frame the last exit's walk materialized, taken, and
    /// whether it stood in optimized code.
    pub(crate) fn take_walked(&mut self) -> Option<(ActivationReference, bool)> {
        let walked = self.walked.take()?;
        Some((walked, std::mem::take(&mut self.walked_optimized)))
    }

    /// The installed body holding the byte before `return_address`.
    fn installed_body_at(&self, return_address: usize) -> Option<&InstalledBody> {
        let index = self.installed_index_at(return_address)?;
        Some(&self.bodies[index as usize])
    }

    /// The index in `bodies` of the installed body holding the byte before
    /// `return_address`, unless it is freed.
    fn installed_index_at(&self, return_address: usize) -> Option<u32> {
        let zone = self.zone.as_ref()?;
        let base = zone.writer.zone().executable_address(CodeOffset(0));
        let offset = u32::try_from(return_address.checked_sub(base)?.checked_sub(1)?).ok()?;
        let holds = |index: usize| {
            self.bodies
                .get(index)
                .is_some_and(|body| body.start <= offset && offset < body.end && !body.free)
        };
        let entry = self.body_lookup.entry(offset);
        if let Some(index) = entry.get().checked_sub(1) {
            if holds(index as usize) {
                return Some(index);
            }
        }
        #[cfg(test)]
        self.body_lookup
            .searches
            .set(self.body_lookup.searches.get() + 1);
        // Installed in ascending order, so the body holding the offset, if
        // one does, is the last that starts at or before it.
        let after = self.bodies.partition_point(|body| body.start <= offset);
        let index = after.checked_sub(1)?;
        if !holds(index) {
            return None;
        }
        let index = u32::try_from(index).ok()?;
        entry.set(index + 1);
        Some(index)
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
        let Ok(zone) = reserve_zone(bytes) else {
            panic!("a zone")
        };
        JitState {
            zone: Some(zone),
            unavailable: None,
            bodies: Vec::new(),
            returns: IdentityMap::default(),
            body_lookup: BodyLookup::default(),
            enabled: true,
            counting: false,
            statistics: JitStatistics::default(),
            install_bodies_exiting_at_once,
            futures_exit: false,
            compile_at_entry: COMPILE_AT_ENTRY,
            compile_at_back_jump: COMPILE_AT_BACK_JUMP,
            #[cfg(ns_jit)]
            walk: materialize::Walk::default(),
            #[cfg(ns_jit)]
            cut: cut::CutWalk::default(),
            walked: None,
            walked_optimized: false,
            refused_for_an_uncompiled_callee: false,
            block: EntryBlock::new(),
            block_stale: true,
            compaction_requested: false,
            zone_exhausted: false,
            wanted_bytes: 0,
            held_compiles: 0,
            fruitless_compactions: 0,
            bytes_freed_since_compaction: 0,
            zone_damping: crate::heap::limits::ShrinkDamping::default(),
            #[cfg(debug_assertions)]
            entries_in_progress: 0,
            abandoned: Vec::new(),
            revived_by_entries: false,
            revival_base: REVIVAL_ENTRIES,
            optimization_requests: Vec::new(),
            budget_words_to_point: Vec::new(),
            optimization_budget: OPTIMIZATION_BUDGET,
            budget_set: OPTIMIZATION_BUDGET,
            optimize_stress: false,
            deopt_stress: false,
            invalidate_stress: false,
            #[cfg(ns_jit)]
            stress_back_offs: IdentityMap::default(),
            #[cfg(ns_jit)]
            customized: Vec::new(),
            deoptimization_draw: DEOPTIMIZATION_SEED,
            deoptimization_countdown: 0,
            machine_words: crate::heap::machine_words::MachineWords::new(),
            #[cfg(ns_jit)]
            optimized_copies: None,
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
    /// Whether the body is a twin, which reifies its frame
    /// (`InstalledBody::reifies`).
    pub(crate) reifies: bool,
    /// Whether the code is the body's optimized code (`Tier::Optimized`).
    pub(crate) optimized: bool,
}

/// How many bodies `BodyLookup` remembers: a power of two.
const BODY_LOOKUP_ENTRIES: usize = 256;

/// The bodies the last lookups by an address inside the code found, one
/// per entry an offset hashes to, each its index in `JitState::bodies`
/// plus one, 0 for none. The step, initializer and own-slot leaves are
/// handed the code's own address, which the call returns do not record,
/// and DeltaBlue spent about a sixth of its time in the binary search
/// over the installed bodies their lookups made, a search a twin per
/// closure-making body lengthened. An entry is a hint, never trusted: it
/// answers only when the body it names still holds the offset and is not
/// freed, and only the body holding an offset can, since live installed
/// bodies never overlap -- so a compaction, a free or a replaced zone,
/// which move, free or drop bodies, need not clear it.
struct BodyLookup {
    entries: [Cell<u32>; BODY_LOOKUP_ENTRIES],
    /// The lookups that searched `bodies`, every one the entries did not
    /// answer: kept for the tests alone, so that no other build pays a
    /// store for it on every miss.
    #[cfg(test)]
    searches: Cell<u64>,
}

impl Default for BodyLookup {
    fn default() -> BodyLookup {
        BodyLookup {
            entries: std::array::from_fn(|_| Cell::new(0)),
            #[cfg(test)]
            searches: Cell::new(0),
        }
    }
}

impl BodyLookup {
    /// The entry an offset into the zone hashes to: the offsets of one
    /// body's leaf calls lie a few dozen bytes apart and bodies at least
    /// sixteen, so a multiplicative hash spreads both.
    fn entry(&self, offset: u32) -> &Cell<u32> {
        const SHIFT: u32 = u32::BITS - BODY_LOOKUP_ENTRIES.trailing_zeros();
        // Fibonacci hashing: 2^32 divided by the golden ratio, odd so that
        // the multiply is a bijection, whose top bits spread offsets that
        // differ in their low bits alone, as nearby leaf calls' do.
        const MULTIPLIER: u32 = 0x9E37_79B1;
        let hashed = offset.wrapping_mul(MULTIPLIER) >> SHIFT;
        &self.entries[hashed as usize]
    }
}

/// `body` as `Installed`, while its code and method live. Inline, as
/// `JitState::body_of` is.
#[inline(always)]
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
        optimized: body.tier == Tier::Optimized,
    })
}

/// The paddings a measured build lays its code zone out with, set when it
/// is built (`NS_NEXT_ZONE_STUB_PADDING`, bytes before the first stub, and
/// `NS_NEXT_ZONE_BODY_PADDING`, bytes between the last stub and the first
/// body): the same machine code in another place in the zone runs at
/// another speed, so each of a measurement's builds carries its own
/// (`measuring-performance`). Unset, both are 0 and nothing is compiled in.
#[cfg(ns_jit)]
const ZONE_STUB_PADDING: u32 = padding_from(option_env!("NS_NEXT_ZONE_STUB_PADDING"));
#[cfg(ns_jit)]
const ZONE_BODY_PADDING: u32 = padding_from(option_env!("NS_NEXT_ZONE_BODY_PADDING"));

/// The largest padding a build accepts.
#[cfg(ns_jit)]
const MAXIMUM_ZONE_PADDING: u32 = 4096;

/// `text` as a padding: absent is 0; anything but a multiple of 16 up to
/// `MAXIMUM_ZONE_PADDING` in decimal fails the build.
#[cfg(ns_jit)]
const fn padding_from(text: Option<&str>) -> u32 {
    let Some(text) = text else {
        return 0;
    };
    let digits = text.as_bytes();
    assert!(!digits.is_empty(), "a zone padding is a decimal number");
    let mut value: u32 = 0;
    let mut index = 0;
    while index < digits.len() {
        assert!(
            digits[index].is_ascii_digit(),
            "a zone padding is a decimal number"
        );
        value = value * 10 + (digits[index] - b'0') as u32;
        assert!(
            value <= MAXIMUM_ZONE_PADDING,
            "a zone padding is at most 4096"
        );
        index += 1;
    }
    assert!(
        value.is_multiple_of(16),
        "a zone padding is a multiple of 16"
    );
    value
}

/// A zone of `bytes` with both stubs installed from its stub padding, or why there
/// is none: the reservation's error -- a system without what a zone needs
/// among them (`ns_memory::MemoryError::Unsupported`), which a host reports
/// rather than run interpreted unawares -- or stubs that do not fit.
#[cfg(ns_jit)]
fn reserve_zone(bytes: usize) -> Result<Zone, String> {
    let writer = ns_memory::CodeZone::reserve(bytes).map_err(|error| error.to_string())?;
    zone_with_stubs(writer, bytes)
        .ok_or_else(|| format!("the code zone's stubs do not fit in {bytes} bytes"))
}

/// The zone `writer` writes, installing at most `bytes`, with every stub
/// installed from `stub_padding`; `None` when they do not fit.
#[cfg(ns_jit)]
fn zone_with_stubs(writer: ns_memory::CodeWriter, bytes: usize) -> Option<Zone> {
    zone_with_paddings(writer, bytes, ZONE_STUB_PADDING, ZONE_BODY_PADDING)
}

/// `zone_with_stubs`, the first stub `stub_padding` bytes into the zone and
/// the first body `body_padding` bytes past the last stub. The first body
/// starts there after a compaction and in a replaced zone too, since both
/// start from `bodies_start`.
#[cfg(ns_jit)]
fn zone_with_paddings(
    writer: ns_memory::CodeWriter,
    bytes: usize,
    stub_padding: u32,
    body_padding: u32,
) -> Option<Zone> {
    use macro_assembler::Abi;

    let limit = u32::try_from(bytes.min(writer.zone().size())).ok()?;
    let mut zone = Zone {
        writer,
        limit,
        next_offset: stub_padding,
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
        dispatching_stub: CodeOffset(0),
        counting_dispatching_stub: CodeOffset(0),
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
    zone.dispatching_stub = install(
        &mut zone,
        &targets::assemble_dispatching_stub(Abi::host(), false).bytes,
    )
    .ok()?;
    zone.counting_dispatching_stub = install(
        &mut zone,
        &targets::assemble_dispatching_stub(Abi::host(), true).bytes,
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
    zone.bodies_start = zone
        .next_offset
        .checked_add(body_padding)
        .filter(|start| *start <= zone.limit)?;
    zone.next_offset = zone.bodies_start;
    Some(zone)
}

#[cfg(not(ns_jit))]
fn reserve_zone(_bytes: usize) -> Result<Zone, String> {
    Err("this build has no machine code encoder".to_string())
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
/// this zone compiled against the entry's feedback base -- its optimized
/// code's once it has some (`JitState::entered_body`), whose install
/// writes every cell that named the baseline code afresh
/// (`optimize_requested`): behind a guard that is the receiver's own id,
/// the copy customized for the receiver's application and the one the
/// target's level names above it, once there is one
/// (`receiver_guarded_copy`), and the body's baseline code before; behind
/// a `super` or private lexical guard, S's or N's application word, the
/// body's dispatching entry once it has a customized copy, which finds
/// the receiver's (`dispatching_entry`); behind any other guard the
/// baseline code -- and through the
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
    entry: &crate::interpreter::inline_cache::CacheEntry,
) -> u64 {
    entry_of_target(state, entry.guard, &entry.target, true)
}

/// `cell_entry` of the entry whose guard is `guard` and whose target is
/// `target`, for a holder that keeps the two apart: a global row, and a
/// cell made before its entry.
pub(crate) fn cell_entry_for(
    state: &JitState,
    guard: DispatchId,
    target: &crate::interpreter::inline_cache::Target,
) -> u64 {
    entry_of_target(state, guard, target, true)
}

/// `cell_entry_for` as a check of a cell reads it, raising no body's
/// usage: the verification of the cells (`InlineCaches::verify_cells`)
/// must not change which bodies a compaction frees.
pub(crate) fn expected_cell_entry(
    state: &JitState,
    guard: DispatchId,
    target: &crate::interpreter::inline_cache::Target,
) -> u64 {
    entry_of_target(state, guard, target, false)
}

/// The copy of `code` a cell guarded by `guard` names for a method target
/// found on `chain`, `level` superclass steps above the chain's start:
/// only a chain that starts at the receiver's class has the receiver's
/// own id as its guard, and so a receiver's application and, `level`
/// above it, the entered one -- the key's two halves. A `super` or
/// private lexical site's guard is the application of the class its
/// chain starts at, which tells nothing of the receiver.
fn receiver_guarded_copy<'state>(
    state: &'state JitState,
    code: &CompiledCode,
    guard: DispatchId,
    chain: crate::interpreter::resolve::Chain,
    level: u16,
) -> Option<&'state InstalledBody> {
    use crate::interpreter::resolve::Chain;
    if !matches!(chain, Chain::Public | Chain::Protected) || !code.jit.has_copies() {
        return None;
    }
    let key = code
        .jit
        .copies
        .borrow()
        .iter()
        .find(|copy| copy.key.receiver == guard && copy.level == Some(level))
        .map(|copy| copy.key)?;
    state.copy_of(code, key)
}

/// The dispatching entry of `code`'s lean body that a cell guarded on
/// `chain` names for a method target found against `feedback_base`,
/// raising the body's usage when `noting`: only
/// a `super` or private lexical chain's guard is the application word of
/// the class its chain starts at, which tells nothing of the receiver, so
/// such a cell names the entry once the body has a customized copy, and
/// its baseline code before (`targets::assemble_dispatching_stub`). The
/// entry lies in the lean body, whose usage the cell raises as a cell
/// naming its start does, and which a compaction moves it with.
#[cfg(ns_jit)]
fn dispatching_entry(
    state: &JitState,
    code: &CompiledCode,
    (chain, feedback_base): (crate::interpreter::resolve::Chain, FeedbackBase),
    noting: bool,
) -> Option<u64> {
    use crate::interpreter::resolve::Chain;
    if !matches!(chain, Chain::Private | Chain::Super) || !code.jit.has_copies() {
        return None;
    }
    let lean = state.installed(code)?;
    if lean.feedback_base != feedback_base || lean.dispatching.is_none() {
        return None;
    }
    // `copy_of` for each row, the body installed already: a row names
    // its own copy.
    let has_a_copy = code
        .jit
        .copies
        .borrow()
        .iter()
        .any(|copy| copy.body != NO_BODY && state.live_body(copy.body).is_some());
    if !has_a_copy {
        return None;
    }
    if noting {
        lean.note_linked();
    }
    state.dispatching_entry_address(code)
}

/// The dispatching entry `called` enters `code`'s lean body at, compiled
/// against `feedback_base`, while one to `DISPATCHING_ROWS` keys' rows
/// name an installed copy, so that every installed copy stands in a
/// dispatching row: no row is looked at further, since a row whose copy
/// is gone stands in no dispatching row and so sends the call to the
/// baseline code, which is what the call would enter without it. A body
/// with more copies is called through the key's lookup instead, which
/// reaches every copy (`called`). No cell is written from it.
#[cfg(ns_jit)]
fn called_dispatching_entry(
    state: &JitState,
    code: &CompiledCode,
    feedback_base: FeedbackBase,
) -> Option<u64> {
    let copies = code.jit.installed_copies();
    if copies == 0 || copies > crate::heap::optimized_copies::DISPATCHING_ROWS {
        return None;
    }
    let lean = state.installed(code)?;
    let dispatching = lean
        .dispatching
        .filter(|_| lean.feedback_base == feedback_base)?;
    let zone = state.zone.as_ref()?;
    Some(
        zone.writer
            .zone()
            .executable_address(CodeOffset(lean.start + dispatching.entry)) as u64,
    )
}

/// No body has a dispatching entry without a JIT.
#[cfg(not(ns_jit))]
fn dispatching_entry(
    _state: &JitState,
    _code: &CompiledCode,
    _target: (crate::interpreter::resolve::Chain, FeedbackBase),
    _noting: bool,
) -> Option<u64> {
    None
}

/// `cell_entry_for`, raising the usage of the body it answers the
/// machine entry of when `noting`.
fn entry_of_target(
    state: &JitState,
    guard: DispatchId,
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
            level,
        } if runs_in_machine_code(method, *chain) => {
            if let Some(entry) =
                dispatching_entry(state, &method.code, (*chain, *feedback_base), noting)
            {
                return entry;
            }
            let body = receiver_guarded_copy(state, &method.code, guard, *chain, *level)
                .or_else(|| state.entered_body(&method.code, None))
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
            return match state.entered_body(code, None) {
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

    /// The address of the refusing stub, 0 where there is no zone.
    pub(crate) fn refusing_stub_address(&self) -> u64 {
        self.zone.as_ref().map_or(0, |zone| {
            zone.writer.zone().executable_address(zone.refusing_stub) as u64
        })
    }

    /// The address of the shared tail of every method body's dispatching
    /// entry (`targets::assemble_dispatching_stub`), 0 where there is no
    /// zone.
    pub(crate) fn dispatching_stub_address(&self) -> u64 {
        self.zone.as_ref().map_or(0, |zone| {
            zone.writer.zone().executable_address(zone.dispatching_stub) as u64
        })
    }

    /// The address of the dispatching entry's tail a body compiled with
    /// `counting` jumps to: the one that counts the calls it enters a copy
    /// by, or the shared tail; 0 where there is no zone.
    pub(crate) fn dispatching_tail_address(&self, counting: bool) -> u64 {
        if !counting {
            return self.dispatching_stub_address();
        }
        self.zone.as_ref().map_or(0, |zone| {
            zone.writer
                .zone()
                .executable_address(zone.counting_dispatching_stub) as u64
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

    /// The one uncustomized copy of optimized code installed beside the
    /// baseline code `installed` answers for `code`, if it has one.
    fn optimized(&self, code: &CompiledCode) -> Option<&InstalledBody> {
        self.installed(code)?;
        let index = code.jit.optimized.get();
        if index == NO_BODY {
            return None;
        }
        let body = self.live_body(index)?;
        debug_assert_eq!(
            body.tier,
            Tier::Optimized,
            "internal: a slot's optimized code is optimized"
        );
        Some(body)
    }

    /// The customized copy of `code`'s optimized code installed for
    /// `key` beside the baseline code `installed` answers for it, if it
    /// has one.
    fn copy_of(&self, code: &CompiledCode, key: CustomizationKey) -> Option<&InstalledBody> {
        self.installed(code)?;
        let index = code.jit.copy_row(key)?.body;
        if index == NO_BODY {
            return None;
        }
        let body = self.live_body(index)?;
        debug_assert!(
            body.tier == Tier::Optimized && body.key == Some(key),
            "internal: a copy's row names its key's optimized code"
        );
        Some(body)
    }

    /// The optimized code an activation of `code` with `key` -- `None`
    /// for one whose key is not asked -- runs: the copy for its key once
    /// it has one, the body's one uncustomized copy otherwise, if it has
    /// that.
    fn optimized_for(
        &self,
        code: &CompiledCode,
        key: Option<CustomizationKey>,
    ) -> Option<&InstalledBody> {
        key.and_then(|key| self.copy_of(code, key))
            .or_else(|| self.optimized(code))
    }

    /// Whether `code` has optimized code of any kind here.
    fn has_optimized_code(&self, code: &CompiledCode) -> bool {
        self.optimized(code).is_some()
            || code
                .jit
                .copies
                .borrow()
                .iter()
                .any(|copy| copy.body != NO_BODY && self.copy_of(code, copy.key).is_some())
    }

    /// The machine code a push, a compiled call and a machine entry row
    /// enter `code` at for an activation with `key`: its optimized code for
    /// the key once it has some (`optimized_for`), its baseline code
    /// otherwise. Each starts with the same prologue and takes the same
    /// frame, so that whoever enters at the start needs no other change.
    /// Inline, since an interpreted send into machine code asks it on
    /// every call (`jit::called`), almost always without a key: a call
    /// out of line took 0.4% of CD's samples, a body without copies
    /// included.
    #[inline]
    fn entered_body(
        &self,
        code: &CompiledCode,
        key: Option<CustomizationKey>,
    ) -> Option<&InstalledBody> {
        match key {
            None => self.optimized(code).or_else(|| self.installed(code)),
            Some(key) => self.entered_copy(code, key),
        }
    }

    /// `entered_body` for `key`: `optimized_for`'s answer, else
    /// `installed`'s, with the slot's state read once.
    #[inline(never)]
    fn entered_copy(&self, code: &CompiledCode, key: CustomizationKey) -> Option<&InstalledBody> {
        let baseline = self.installed(code)?;
        let copy = code.jit.copy_row(key).map_or(NO_BODY, |row| row.body);
        let optimized = [copy, code.jit.optimized.get()]
            .into_iter()
            .filter(|&index| index != NO_BODY)
            .find_map(|index| self.live_body(index));
        debug_assert!(
            optimized.is_none_or(|body| body.tier == Tier::Optimized),
            "internal: a slot's optimized code is optimized"
        );
        Some(optimized.unwrap_or(baseline))
    }

    /// Where a compiled call enters `code`'s machine code for an
    /// activation with `key` -- its prologue, the body's first byte, its
    /// optimized code's for the key once it has some -- when it has
    /// machine code in this zone compiled against `feedback_base`.
    #[inline]
    fn machine_entry_of(
        &self,
        code: &CompiledCode,
        feedback_base: FeedbackBase,
        key: Option<CustomizationKey>,
    ) -> Option<CodeOffset> {
        let body = self.entered_body(code, key)?;
        (body.feedback_base == feedback_base).then_some(CodeOffset(body.start))
    }

    /// Where the run loop's push enters `code`'s optimized code for an
    /// activation with `key`, past its prologue, for an activation with
    /// `feedback_base`, when it has some.
    fn optimized_push_entry(
        &self,
        code: &CompiledCode,
        feedback_base: FeedbackBase,
        key: Option<CustomizationKey>,
    ) -> Option<CodeOffset> {
        let body = self.optimized_for(code, key)?;
        (body.feedback_base == feedback_base).then_some(CodeOffset(body.start + body.push_entry))
    }

    /// Where the run loop enters `code`'s baseline code at the instruction
    /// at `program_counter`, for an activation with `feedback_base`: a
    /// frame that goes on mid-body enters the only code with an entry at
    /// every instruction, and a push of a body with optimized code enters
    /// that instead (`optimized_push_entry`). Program counter 0 is read
    /// off the slot rather than searched for in the map.
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
    /// Answers whether one was revived, here or by its entries since the
    /// last such answer (`note_abandoned_entry`), in which case the caller
    /// writes the cells that name the refusing stub afresh from their
    /// entries (`InlineCaches::refresh_all_cells`),
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
    /// them; the pace is allocation's, and a run that allocates nothing
    /// revives by entries instead. The JIT stress mode never abandons a body
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
            // Optimized code's own count, halved as its baseline code's
            // is, so that a rare exit of it does not add up to its
            // abandonment over a long run either.
            if body.tier == Tier::Optimized && !body.free {
                body.exits.set(body.exits.get() / 2);
            }
        }
        let mut revived = 0;
        self.abandoned.retain(|body| {
            let Some(code) = body.upgrade() else {
                return false;
            };
            // A body refused since -- entered while the JIT was off -- is
            // no longer abandoned.
            if !code.jit.is_abandoned() {
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
        revived > 0 || std::mem::take(&mut self.revived_by_entries)
    }

    /// Counts an interpreted entry of the abandoned body `code`, and
    /// revives it at the last its abandonment waits for
    /// (`revival_entries`): the path to revival that needs no allocation,
    /// so that a run that never scavenges does not keep a lifetime budget.
    /// Deterministic, since it counts entries, never time. The count is the
    /// slot's state (`FIRST_ABANDONED`), which a push of the body adds to
    /// inline (`JitSlot::push_is_armed`); the misses that count the entry of
    /// a factory come here for each. The cells that refuse its calls are
    /// written afresh by the trigger's caller, which holds the mutator
    /// (`after_compile_trigger`).
    fn note_abandoned_entry(&mut self, code: &Rc<CompiledCode>) {
        let slot = &code.jit;
        let state = slot.state.get();
        if state < LAST_ABANDONED {
            slot.state.set(state + 1);
            return;
        }
        slot.forget();
        let revived = Rc::as_ptr(code);
        self.abandoned
            .retain(|body| !std::ptr::eq(body.as_ptr(), revived));
        self.statistics.revived_bodies += 1;
        self.revived_by_entries = true;
    }

    /// Whether a body was revived by its entries since the cells were last
    /// written afresh for a revival, which the caller now does
    /// (`InlineCaches::refresh_all_cells`).
    pub(crate) fn take_revival(&mut self) -> bool {
        std::mem::take(&mut self.revived_by_entries)
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

/// The address a compiled call enters `code` at for an activation with
/// `key` (`entry_key`), its machine entry, when it has machine code in
/// this zone compiled against `feedback_base`. Inline, as `entered_body`
/// is, for the interpreted sends into machine code (`called`,
/// `called_closure`).
#[inline]
pub(crate) fn machine_entry_address(
    state: &JitState,
    code: &CompiledCode,
    feedback_base: FeedbackBase,
    key: Option<CustomizationKey>,
) -> Option<u64> {
    let entry = state.machine_entry_of(code, feedback_base, key)?;
    let zone = state.zone.as_ref()?;
    Some(zone.writer.zone().executable_address(entry) as u64)
}

/// The key an entry into `code` on `receiver` at `current_class` asks
/// its copies for: `None` for a body with no copy installed for any key
/// (`JitSlot::has_installed_copy`), whose entry answers the same whatever
/// the key, so that only an entry into a customized body pays for the
/// lookups.
pub(crate) fn entry_key<'gc>(
    heap: &Heap<'gc>,
    code: &CompiledCode,
    receiver: Value<'gc>,
    current_class: Option<ClassReference<'gc>>,
) -> Option<CustomizationKey> {
    if !code.jit.has_installed_copy() {
        return None;
    }
    CustomizationKey::of(heap, receiver, current_class)
}

/// `entry_key` for `activation`, read off its frame's header.
fn activation_key(
    heap: &Heap<'_>,
    code: &CompiledCode,
    activation: ActivationReference,
) -> Option<CustomizationKey> {
    if !code.jit.has_installed_copy() {
        return None;
    }
    let (receiver, current_class) = heap.activation_receiver_and_current_class(activation);
    CustomizationKey::of(heap, receiver, current_class)
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
/// A caller that holds the mutator then calls `after_compile_trigger`,
/// which passes the compaction a full zone asks for on to the heap's
/// safepoint request and writes the cells a revival changed.
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
        FIRST_ABANDONED..=LAST_ABANDONED => state.note_abandoned_entry(code),
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
            REFUSED | EXITS_AT_ONCE | FIRST_ABANDONED..=LAST_ABANDONED => false,
            COMPILED => code.jit.zone.get() != state.zone_identity(),
            entries => entries + 1 >= state.compile_at_entry,
        };
    if compiles {
        note_entry(state, code, feedback_base, method);
    }
}

/// Notes that the miss leaf refuses a compiled call to `code`, which has
/// no machine code to enter: a refusal that says nothing of the caller's
/// code when `code` is counting its entries, or compiled into another
/// zone, and the zone has room to compile it at an entry to come, which
/// the exit then counts against no budget (`resumption_after_exit`).
/// Every other refusal counts: a callee compiled here against another
/// feedback base, which no call compiles again, and one whose compile the
/// exhausted zone holds, whose retries back off for many entries -- a hot
/// caller of either would exit at every call, and its abandonment is what
/// stops that, and gives the exhausted zone its room back.
pub(crate) fn note_call_refused(state: &mut JitState, code: &CompiledCode) {
    let slot = &code.jit;
    let counting = match slot.state.get() {
        COMPILED => slot.zone.get() != state.zone_identity(),
        entries => entries < COMPILED,
    };
    state.refused_for_an_uncompiled_callee =
        state.enabled && counting && state.may_compile_now(slot);
}

/// Compiles `code` into the zone and records the outcome in its slot and
/// the statistics.
fn compile_into_slot(
    state: &mut JitState,
    code: &Rc<CompiledCode>,
    feedback_base: FeedbackBase,
    method: Option<&Rc<MixinMethod>>,
) {
    let started = std::time::Instant::now();
    let outcome = compile(state, code, feedback_base, method);
    state.statistics.count_compile_time(started);
    match outcome {
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
        Err(Refusal::TooManyInstructions | Refusal::Unassembled(_) | Refusal::TooLarge) => {
            code.jit.refuse();
            state.statistics.refused_bodies += 1;
        }
    }
}

/// What follows every compile trigger that holds the mutator -- a push, a
/// miss, an initializer invocation, a future forced -- since `note_entry`
/// itself reaches only the JIT's state: the one place, so that no trigger
/// can skip either part.
/// - A compaction a compile asked for (`JitState::compaction_requested`)
///   is passed on to the heap, whose safepoint request the run loop polls.
///   Only a request: a leaf may call this, and the compaction waits for the
///   run loop's safepoint, where no return address into the zone exists.
/// - A body the trigger's entry revived (`JitState::note_abandoned_entry`)
///   has every cell that names the refusing stub written afresh from its
///   entry (`InlineCaches::refresh_all_cells`), so that every caller's cell
///   of it names the miss stub again, not only the site that counted. A
///   leaf may write cells, as the miss leaf writes its own site's.
/// - Each body the trigger installed has its budget word pointed at its
///   row, armed with `OPTIMIZATION_BUDGET` unless it is armed already
///   (`point_budget_words`), before its code first runs.
///
/// Every initializer invocation a leaf decides passes through here,
/// nearly always with nothing installed: only the test for an installed
/// body stays inline, so that the trigger inlines into its callers and
/// writes nothing into the JIT's state when nothing was installed.
pub(crate) fn after_compile_trigger(mutator: &mut Mutator<'_>) {
    #[cfg(ns_jit)]
    if !mutator.jit.budget_words_to_point.is_empty() {
        point_budget_words(mutator);
    }
    if mutator.jit.compaction_requested {
        mutator.heap.request_safepoint();
    }
    if mutator.jit.take_revival() {
        mutator.caches.refresh_all_cells(&mutator.jit);
    }
}

/// Compiles `code`, its sends' cells those of `feedback_base`'s range,
/// and installs its lean body, answering its offset and that of its
/// instruction at program counter 0 and its index among the installed
/// bodies. Its twin waits for the first transfer or resumption that needs
/// it (`compile_twin`).
#[cfg(ns_jit)]
fn compile(
    state: &mut JitState,
    code: &Rc<CompiledCode>,
    feedback_base: FeedbackBase,
    method: Option<&Rc<MixinMethod>>,
) -> Result<([CodeOffset; 2], u32), Refusal> {
    let settings = CompileSettings {
        counting: state.counting,
        futures_exit: state.futures_exit,
    };
    let body = assemble(
        macro_assembler::Abi::host(),
        settings,
        code,
        feedback_base,
        method,
        baseline::BodyCopy::Lean,
    )?;
    if body.exits_at_once && !state.install_bodies_exiting_at_once {
        return Err(Refusal::ExitsAtOnce);
    }
    // Every body has an instruction, its last a return or a jump, so a
    // map without program counter 0 is the JIT's own defect.
    let Some(first_instruction) = body.map.machine_offset(0) else {
        panic!("internal: a compiled body has no code for its first instruction");
    };
    let needed = needed_bytes(&body);
    let map = match install_body(state, code, method, feedback_base, body, settings, None) {
        Ok(map) => map,
        Err(Refusal::ZoneFull) => {
            state.wanted_bytes = state.wanted_bytes.max(needed);
            code.jit.needed.set(needed);
            return Err(Refusal::ZoneFull);
        }
        Err(refusal) => return Err(refusal),
    };
    let offset = CodeOffset(state.bodies[map as usize].start);
    let first_instruction = CodeOffset(offset.0 + first_instruction);
    Ok(([offset, first_instruction], map))
}

/// `code` assembled for `abi` as its body's `copy` under `settings`, for
/// `method` when it is one's body, its sends' cells those of
/// `feedback_base`'s range. The JIT assembles for the host's convention;
/// a test assembles for another's, so that no host is needed to find an
/// operation its encoder cannot lay out.
#[cfg(ns_jit)]
fn assemble(
    abi: macro_assembler::Abi,
    settings: CompileSettings,
    code: &CompiledCode,
    feedback_base: FeedbackBase,
    method: Option<&Rc<MixinMethod>>,
    copy: baseline::BodyCopy,
) -> Result<baseline::BaselineBody, Refusal> {
    let mut macro_assembler = macro_assembler::MacroAssembler::new(abi);
    macro_assembler.set_count_back_edges(settings.counting);
    let role = baseline::BodyRole::of(code, method.is_some());
    baseline::compile(
        code,
        macro_assembler,
        feedback_base,
        role,
        settings.futures_exit,
        copy,
    )
}

/// The bytes `body` needs of the zone, its alignment's slack included:
/// what a compile that found no room records, so that an exhausted zone
/// assembles it again only once it has that much (`may_compile_now`).
#[cfg(ns_jit)]
fn needed_bytes(body: &baseline::BaselineBody) -> u32 {
    u32::try_from(body.code.bytes.len())
        .unwrap_or(u32::MAX)
        .saturating_add(body.code.alignment.max(BODY_ALIGNMENT))
}

/// Installs `body`, a compile of `code` -- `method`'s body when it is
/// one's -- against `feedback_base`, and records its call returns,
/// answering its index among the installed bodies: `Refusal::TooLarge`
/// for a body the zone could not hold even with every other body freed,
/// and `Refusal::ZoneFull` for one the room left does not hold. `baseline`
/// names the lean body whose optimized code `body` is, `None` for baseline
/// code.
#[cfg(ns_jit)]
fn install_body(
    state: &mut JitState,
    code: &Rc<CompiledCode>,
    method: Option<&Rc<MixinMethod>>,
    feedback_base: FeedbackBase,
    body: baseline::BaselineBody,
    settings: CompileSettings,
    baseline: Option<u32>,
) -> Result<u32, Refusal> {
    let Some(zone) = state.zone.as_mut() else {
        panic!("internal: the JIT is on without a zone");
    };
    let map = u32::try_from(state.bodies.len()).map_err(|_| Refusal::ZoneFull)?;
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
    // The body's data words, its closure sites and its transfers among
    // them, are installed with it, zeroed, so that each is filled at its
    // first run; a zone too full for the code and its words refuses both,
    // and the compaction that follows frees room for them at least.
    let offset = install_aligned(zone, &body.code.bytes, body.code.alignment)?;
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
    let words = body.words.as_ref().map_or(length, |words| words.start);
    let twin_transfers = body
        .twin_transfers
        .iter()
        .map(|transfer| {
            let Some(offset) = u32::try_from(transfer.word)
                .ok()
                .and_then(|word| word.checked_mul(8))
                .and_then(|bytes| bytes.checked_add(words))
            else {
                unreachable!("internal: a body's data words lie within its bytes");
            };
            TwinTransferWord {
                program_counter: transfer.program_counter,
                offset,
            }
        })
        .collect();
    let budget_word = body.budget_word.map(|word| {
        let Some(offset) = u32::try_from(word)
            .ok()
            .and_then(|word| word.checked_mul(8))
            .and_then(|bytes| bytes.checked_add(words))
        else {
            unreachable!("internal: a body's data words lie within its bytes");
        };
        offset
    });
    let unpointed = budget_word.map(|_| state.unpointed_budget_address());
    let push_entry = body.map.machine_offset(0).unwrap_or(0);
    let word_offset = |word: usize| {
        let Some(offset) = u32::try_from(word)
            .ok()
            .and_then(|word| word.checked_mul(8))
            .and_then(|bytes| bytes.checked_add(words))
        else {
            unreachable!("internal: a body's data words lie within its bytes");
        };
        offset
    };
    let dispatching = body.dispatching.map(|dispatching| InstalledDispatching {
        entry: dispatching.entry,
        rows_word: word_offset(dispatching.rows_word),
        tail_word: word_offset(dispatching.tail_word),
        rows: 0,
    });
    state.bodies.push(InstalledBody {
        code: Rc::downgrade(code),
        method: method.map(Rc::downgrade),
        feedback_base,
        reifies: body.reifies,
        twin: None,
        lean: None,
        twin_transfers,
        twin_needed: 0,
        settings,
        start: offset.0,
        end: offset.0 + length,
        words: offset.0 + words,
        words_end: offset.0 + body.words.as_ref().map_or(length, |words| words.end),
        map: body.map,
        alignment: body.code.alignment.max(BODY_ALIGNMENT),
        usage: Cell::new(zone::INITIAL_USAGE),
        free: false,
        budget_word,
        tier: if baseline.is_some() {
            Tier::Optimized
        } else {
            Tier::Baseline
        },
        baseline,
        push_entry,
        exits: Cell::new(0),
        optimized_needed: 0,
        key: None,
        dispatching,
    });
    // Until it is pointed at its row the code spends from the unpointed
    // budget, whose first spend calls the leaf: the word is never one the
    // spend must test for.
    if let Some(unpointed) = unpointed {
        state.write_budget_word(map, unpointed);
        state.budget_words_to_point.push(map);
    }
    // Likewise its dispatching entry finds no copy until its word is
    // pointed with the budget's (`point_budget_word`); the tail never
    // moves.
    if dispatching.is_some() {
        let vacant = state.copies_table().vacant_address();
        let tail = state.dispatching_tail_address(settings.counting);
        state.write_dispatching_words(map, Some(vacant), Some(tail));
    }
    Ok(map)
}

/// Compiles the twin of the lean body at `lean` in `bodies` -- the same
/// instructions compiled to reify their frame (`baseline::BodyCopy::Twin`)
/// -- installs it, and writes the lean body's transfer words with the
/// twin's address of each `MakeClosure`, answering the twin's index; the
/// twin's, if it is compiled already. Called by the twin leaf at a
/// transfer and by the resumption of a reified frame, both before
/// anything of the frame is written; an install only appends, so no
/// address a frame or a holder keeps moves. A zone with no room for the
/// twin is treated as for any body that finds none (`compile_into_slot`):
/// a compaction is asked for, and while the zone stays exhausted a twin
/// known not to fit the room left is not assembled again.
#[cfg(ns_jit)]
fn compile_twin(state: &mut JitState, lean: u32) -> Result<u32, Refusal> {
    let Some(body) = state.live_body(lean) else {
        unreachable!("internal: a twin is compiled for a live lean body");
    };
    debug_assert!(body.lean.is_none(), "internal: a twin has no twin");
    if let Some(twin) = body.twin {
        return Ok(twin);
    }
    let (needed, settings) = (body.twin_needed, body.settings);
    let Some(installed) = upgraded(body) else {
        unreachable!("internal: a lean body running or resumed lives");
    };
    if installed.code.jit.twin_refused.get() {
        return Err(Refusal::TooLarge);
    }
    if !state.may_compile_needing(needed) {
        state.hold_compile();
        return Err(Refusal::ZoneFull);
    }
    let started = std::time::Instant::now();
    let twin = match assemble(
        macro_assembler::Abi::host(),
        settings,
        &installed.code,
        installed.feedback_base,
        installed.method.as_ref(),
        baseline::BodyCopy::Twin,
    ) {
        Ok(twin) => twin,
        Err(refusal) => {
            state.statistics.count_compile_time(started);
            installed
                .code
                .jit
                .twin_refused
                .set(refusal != Refusal::ZoneFull);
            return Err(refusal);
        }
    };
    let needed = needed_bytes(&twin);
    let installation = install_body(
        state,
        &installed.code,
        installed.method.as_ref(),
        installed.feedback_base,
        twin,
        settings,
        None,
    );
    state.statistics.count_compile_time(started);
    let index = match installation {
        Ok(index) => index,
        Err(Refusal::ZoneFull) => {
            state.wanted_bytes = state.wanted_bytes.max(needed);
            state.bodies[lean as usize].twin_needed = needed;
            if state.zone_exhausted {
                state.hold_compile();
            } else {
                state.compaction_requested = true;
                state.zone_exhausted = true;
            }
            return Err(Refusal::ZoneFull);
        }
        Err(refusal) => {
            installed.code.jit.twin_refused.set(true);
            return Err(refusal);
        }
    };
    // A twin is as used as its lean body: the pair is freed as one.
    let usage = state.bodies[lean as usize].usage.get();
    let twin_body = &mut state.bodies[index as usize];
    twin_body.lean = Some(lean);
    twin_body.usage.set(usage);
    let lean_body = &mut state.bodies[lean as usize];
    lean_body.twin = Some(index);
    lean_body.twin_needed = 0;
    state.write_pair_transfer_words(lean);
    state.statistics.compiled_twins += 1;
    Ok(index)
}

/// The twin leaf's work (`leaves::twin`): the twin of the lean body the
/// address inside the code `code_address` lies in compiled, unless it is,
/// answering whether it is.
#[cfg(ns_jit)]
pub(crate) fn compile_twin_at(state: &mut JitState, code_address: usize) -> bool {
    let Some(index) = state.installed_index_at(code_address) else {
        unreachable!("internal: a transfer lies in an installed body");
    };
    // Optimized code transfers into its baseline body's twin, whose
    // address its own words hold as the lean body's do.
    // Its words name the twin from the twin's compile on, as the lean
    // body's do (`write_pair_transfer_words`).
    let optimized = state.bodies[index as usize].baseline;
    if optimized.is_some() {
        state.statistics.optimized_twin_requests += 1;
    }
    compile_twin(state, optimized.unwrap_or(index)).is_ok()
}

/// Points the budget word of every body installed since the last trigger
/// at its body's row (`point_budget_word`), before its code first runs
/// (`after_compile_trigger`).
#[cfg(ns_jit)]
#[cold]
#[inline(never)]
fn point_budget_words(mutator: &mut Mutator<'_>) {
    for index in std::mem::take(&mut mutator.jit.budget_words_to_point) {
        point_budget_word(mutator, index, None);
    }
}

/// What is left of each installed body's armed budget, by the body's
/// address, read before a full collection's `reclaim_ids` frees the ids
/// the rows are indexed by and clears their rows: the after-collection
/// pass arms each body's row of the id it has then with it
/// (`point_budget_words_after_collection`), so that a body whose id was
/// freed -- every method no activation runs -- keeps the budget it spent
/// rather than starting afresh at every collection.
#[cfg(ns_jit)]
pub(crate) fn budgets_before_collection(mutator: &Mutator<'_>) -> IdentityMap<usize, i64> {
    let mut budgets = IdentityMap::default();
    for body in mutator.jit.bodies.iter().filter(|body| !body.free) {
        if let Some(code) = body.code.upgrade() {
            if let Some(budget) = mutator.heap.armed_optimization_budget(&code) {
                budgets.insert(Rc::as_ptr(&code) as usize, budget);
            }
        }
    }
    budgets
}

/// Without a JIT no body has a budget.
#[cfg(not(ns_jit))]
pub(crate) fn budgets_before_collection(_mutator: &Mutator<'_>) -> IdentityMap<usize, i64> {
    IdentityMap::default()
}

/// Points the budget word of every installed body afresh after a full
/// collection's `reclaim_ids`, which may have freed the code ids the rows
/// are indexed by, and before the table gives the rows past the code
/// table back (`World::collect`): the id of a body that lives is asked of
/// the heap again -- minted afresh for one whose id was freed, which only
/// what holds the body keeps alive, so that no body outlives its holders
/// for its machine code's sake -- and its row armed with what was left of
/// its budget, `carried` (`budgets_before_collection`); a dead body's
/// word names the unpointed budget.
#[cfg(ns_jit)]
pub(crate) fn point_budget_words_after_collection(
    mutator: &mut Mutator<'_>,
    carried: &IdentityMap<usize, i64>,
) {
    mutator.jit.budget_words_to_point.clear();
    let unpointed = mutator.jit.unpointed_budget_address();
    for index in 0..mutator.jit.bodies.len() {
        let index = u32::try_from(index).expect("internal: a body's index fits a u32");
        if !point_budget_word(mutator, index, Some(carried)) {
            mutator.jit.write_budget_word(index, unpointed);
            // A dead body's word may name a block whose id was minted for
            // another body.
            mutator.jit.point_dispatching_rows(index, None);
        }
    }
}

/// Without a JIT no body spends a budget.
#[cfg(not(ns_jit))]
pub(crate) fn point_budget_words_after_collection(
    _mutator: &mut Mutator<'_>,
    _carried: &IdentityMap<usize, i64>,
) {
}

/// Points the budget word of the body at `index` in `bodies` at the row
/// of the id its body has now -- the sink for an id past the table --
/// arming the row unless it is armed already: with what `carried` keeps
/// for the body across a collection, or else the state's budget; and a
/// method's lean body's dispatching rows' word at the block of the same
/// id (`JitState::point_dispatching_rows`), for the same reason. Answers
/// whether the body lives to have a row.
#[cfg(ns_jit)]
fn point_budget_word(
    mutator: &mut Mutator<'_>,
    index: u32,
    carried: Option<&IdentityMap<usize, i64>>,
) -> bool {
    let Some(code) = mutator
        .jit
        .live_body(index)
        .and_then(|body| body.code.upgrade())
    else {
        return false;
    };
    let budget = mutator.heap.optimization_budget_of(&code);
    if !budget.armed && !budget.sink {
        let left = carried
            .and_then(|carried| carried.get(&(Rc::as_ptr(&code) as usize)))
            .copied()
            .unwrap_or(mutator.jit.optimization_budget);
        mutator.heap.arm_optimization_budget(budget.id, left);
    }
    mutator.jit.write_budget_word(index, budget.address);
    mutator
        .jit
        .point_dispatching_rows(index, Some(budget.id.index()));
    true
}

/// The budget leaf's work (`leaves::budget`): a spend of the body the
/// address inside the code `code_address` lies in left the word at
/// `spent_from` below zero. A row that ran out -- the one the body's id
/// names now, and armed -- requests the body's optimization, once while
/// the request is pending, parks the row and requests a safepoint, at
/// which the request is taken up. Anything else spent from a word that
/// names no budget of this body's: the unpointed budget, written back
/// negative, or a row cleared since, and the word is pointed at the
/// body's row, armed afresh; the sink an id past the table spends from is
/// parked again.
///
/// A method's body is optimized per key (`CustomizationKey`), its budget
/// still one per body: a run-out requests the copy for the key of the
/// spending frame, at `frame_address`, read off the header its call
/// wrote (`request_copy`).
#[cfg(ns_jit)]
pub(crate) fn budget_spent(
    mutator: &mut Mutator<'_>,
    spent_from: u64,
    frame_address: u64,
    code_address: usize,
) {
    let Some(index) = mutator.jit.installed_index_at(code_address) else {
        unreachable!("internal: a budget is spent by an installed body");
    };
    let Some(code) = mutator
        .jit
        .live_body(index)
        .and_then(|body| body.code.upgrade())
    else {
        unreachable!("internal: a body running machine code lives");
    };
    if spent_from == mutator.jit.unpointed_budget_address() {
        use crate::heap::machine_words::{MachineWord, UNPOINTED_BUDGET};
        mutator.jit.machine_words.set(
            MachineWord::UnpointedBudget,
            UNPOINTED_BUDGET.cast_unsigned(),
        );
    }
    let budget = mutator.heap.optimization_budget_of(&code);
    if budget.sink {
        mutator.heap.park_optimization_budget_sink();
    }
    if budget.sink || spent_from != budget.address || !budget.armed {
        point_budget_word(mutator, index, None);
        return;
    }
    mutator.heap.park_optimization_budget(budget.id);
    let customized = code.kind == crate::bytecode::CodeKind::Method
        && mutator
            .jit
            .live_body(index)
            .is_some_and(|body| body.method.is_some());
    if customized && request_copy(mutator, &code, frame_address) {
        return;
    }
    if code.jit.optimized_refused.get() {
        rearm_for_other_pairs(mutator, &code);
        return;
    }
    request(mutator, OptimizationRequest::uncustomized(&code));
}

/// After a run-out by a frame of a pair refused for good -- a key's row,
/// or the body's one uncustomized copy -- whose budget `budget_spent`
/// parked: the budget is one per body, so that parked it would starve
/// every other pair of the body for good -- a key waiting for the
/// recompile of a copy made not entrant, and a key that has no copy yet.
/// Armed afresh at the plain wait while the body has a pair not refused,
/// since the run-out says that what any waiting pair waited for was
/// spent; left parked only once the body's one uncustomized copy is
/// refused as well as every key's row, so that a body refused throughout
/// calls no leaf once per budget for the rest of the run. A method body
/// whose uncustomized copy was never refused -- a frame without a key
/// never asked for it -- counts as a pair not refused, and so keeps its
/// budget armed, however many of its keys are.
#[cfg(ns_jit)]
fn rearm_for_other_pairs(mutator: &mut Mutator<'_>, code: &Rc<CompiledCode>) {
    let any_left = !code.jit.optimized_refused.get()
        || code.jit.copies.borrow().iter().any(|copy| !copy.refused);
    if any_left {
        rearm_optimization_budget(mutator, code, 0);
    }
}

/// Records `request` unless one for its body and key is pending, and
/// requests the safepoint that takes it up.
#[cfg(ns_jit)]
fn request(mutator: &mut Mutator<'_>, request: OptimizationRequest) {
    let requests = &mut mutator.jit.optimization_requests;
    requests.retain(|pending| pending.code.strong_count() > 0);
    let pending = requests.iter().any(|pending| {
        std::ptr::eq(pending.code.as_ptr(), request.code.as_ptr()) && pending.key == request.key
    });
    if !pending {
        requests.push(request);
        mutator.jit.statistics.optimization_requests += 1;
        mutator.heap.request_safepoint();
    }
}

/// The run-out of `code`'s budget by the frame at `frame_address`, a
/// method's activation in baseline code: requests the copy for the
/// frame's key -- its receiver's application and the one the frame was
/// entered at, read off the header the call wrote, never another
/// activation's -- unless the key has a copy, whose frames ran baseline
/// code only where no entry could ask for the key (a `super` or private
/// lexical send, a frame switched into its loop), or a refused row. A key
/// with a copy has the budget armed afresh, so that another key's
/// run-out compiles that key's copy; a refused one leaves it parked, as a
/// refused body's is. A key first requested gets its row, which records
/// the level the frame was entered at above the receiver's class.
/// Answers whether the frame had a key: a frame without one -- its
/// receiver or its class of an application with no minted id -- is
/// requested the body's one uncustomized copy.
#[cfg(ns_jit)]
fn request_copy(mutator: &mut Mutator<'_>, code: &Rc<CompiledCode>, frame_address: u64) -> bool {
    let Some((receiver, current_class)) = mutator
        .heap
        .machine_frame_receiver_and_current_class(frame_address, code.argument_count)
    else {
        unreachable!("internal: a budget is spent by a frame in the activation stack");
    };
    let Some(key) = CustomizationKey::of(&mutator.heap, receiver, current_class) else {
        return false;
    };
    match code.jit.copy_row(key) {
        Some(row) if row.refused => {
            rearm_for_other_pairs(mutator, code);
            return true;
        }
        // The next run-out is another key's, which has failed nothing:
        // the plain wait.
        Some(_) if mutator.jit.copy_of(code, key).is_some() => {
            rearm_optimization_budget(mutator, code, 0);
            return true;
        }
        Some(_) => {}
        None => {
            let level = current_class.and_then(|class| levels_to(mutator, receiver, class));
            // A key exists only for a class of an application, whose
            // class objects hold its word.
            let entered_word = current_class
                .and_then(|class| mutator.heap.class_application_word(class))
                .unwrap_or(crate::heap::optimized_copies::VACANT);
            if !code.jit.has_copies() {
                mutator.jit.customized.push(Rc::downgrade(code));
            }
            code.jit.copies.borrow_mut().push(CustomizedCopy {
                key,
                body: NO_BODY,
                version: 0,
                traps: 0,
                refused: false,
                level,
                entered_word,
            });
        }
    }
    let entered = current_class
        .and_then(|class| mutator.heap.class_application(class))
        .map_or_else(Weak::new, |application| Rc::downgrade(&application));
    request(
        mutator,
        OptimizationRequest {
            code: Rc::downgrade(code),
            key: Some(key),
            entered,
        },
    );
    true
}

/// How many superclass steps from `receiver`'s class reach `class`, as a
/// cache entry counts its target's level from a receiver's class
/// (`inline_cache::levels_between`); `None` past `u16::MAX` or off the
/// chain.
#[cfg(ns_jit)]
fn levels_to<'gc>(
    mutator: &Mutator<'gc>,
    receiver: Value<'gc>,
    class: ClassReference<'gc>,
) -> Option<u16> {
    let mut current = mutator.class_of(receiver);
    for steps in 0..=u16::MAX {
        if current.same(class) {
            return Some(steps);
        }
        current = mutator.heap.class_superclass(current)?;
    }
    None
}

/// Arms `code`'s budget afresh at the wait a copy at `version` earned
/// (`optimization_budget_after`). The budget is one per body, shared by
/// its pairs, and an arming replaces what is left of the last: a re-arm
/// at the plain wait for another key -- after a keyed install, or a
/// run-out by a frame of a key with a copy or of a refused pair
/// (`rearm_for_other_pairs`) -- can cut short a retired key's wait of 4
/// to its version. Mostly it cuts nothing: a run-out parked the budget,
/// so the wait it held was spent, by whichever pair's frames. Only an
/// install of a request still pending when another key's copy retired
/// shortens a wait, by its arming. Keeping the longer of the two instead
/// would keep a wait that is no retired key's too -- a cache fill raises a
/// running body's budget for its feedback to settle
/// (`Heap::raise_running_optimization_budget`) -- and hold back the next
/// key's first copy behind it.
#[cfg(ns_jit)]
fn rearm_optimization_budget(mutator: &mut Mutator<'_>, code: &Rc<CompiledCode>, version: u8) {
    let budget = mutator.heap.optimization_budget_of(code);
    let armed = optimization_budget_after(mutator.jit.optimization_budget, version);
    mutator.heap.arm_optimization_budget(budget.id, armed);
}

/// Compiles the optimized code of every body whose budget ran out since
/// the last safepoint (`JitState::optimization_requests`) and installs it
/// beside the body's baseline code (`docs/design/execution.md`, "Planned:
/// the optimizing JIT"). Run at the run loop's safepoint
/// (`Interpreter::safepoint`), between two stretches, where no return
/// address into the zone exists; the compile reads only its snapshot, and
/// the install only appends, so neither collects nor moves code.
#[cfg(ns_jit)]
pub(crate) fn optimize_requested(mutator: &mut Mutator<'_>) {
    for request in std::mem::take(&mut mutator.jit.optimization_requests) {
        if let Some(code) = request.code.upgrade() {
            optimize(mutator, &code, request.key, &request.entered);
        }
    }
}

/// Without a JIT nothing is requested.
#[cfg(not(ns_jit))]
pub(crate) fn optimize_requested(mutator: &mut Mutator<'_>) {
    mutator.jit.optimization_requests.clear();
}

/// `code`'s optimized code compiled and installed, when its baseline code
/// is still installed in this zone and has none beside it: from then on a
/// push, every cell of a method it is the body of and its machine entry
/// row enter the optimized code, its prologue the baseline's, while a
/// frame that goes on mid-body goes on in the baseline code. A body the
/// optimizing compiler refuses stays in baseline code, its budget parked,
/// as its request left it, and the refusal recorded
/// (`JitSlot::optimized_refused`), so that it is not requested again,
/// whatever becomes of its baseline code; one whose code the zone has no
/// room for asks for a compaction and has its budget armed afresh.
///
/// A request with a key compiles the copy for it, entered at `entered`,
/// into the key's row (`JitSlot::copies`), unless the row has a copy or
/// a refusal: its refusal is the row's, and once it is installed the
/// body's budget is armed afresh, so that the next run-out, by another
/// key, compiles that key's copy.
#[cfg(ns_jit)]
fn optimize(
    mutator: &mut Mutator<'_>,
    code: &Rc<CompiledCode>,
    key: Option<CustomizationKey>,
    entered: &Weak<MixinApplication>,
) {
    let done = match key {
        None => code.jit.optimized_refused.get() || mutator.jit.optimized(code).is_some(),
        Some(key) => code
            .jit
            .copy_row(key)
            .is_none_or(|row| row.refused || mutator.jit.copy_of(code, key).is_some()),
    };
    if done {
        return;
    }
    debug_assert!(
        entered.upgrade().is_none_or(|application| {
            key.is_some_and(|key| key.entered == application.dispatch_ids.instances)
        }),
        "internal: a request's entered application is its key's"
    );
    let Some(body) = mutator.jit.installed(code) else {
        return;
    };
    // A twin is never named by its slot: `installed` answers the lean body.
    let (lean, start, feedback_base, settings, needed) = (
        code.jit.map.get(),
        body.start,
        body.feedback_base,
        body.settings,
        body.optimized_needed,
    );
    // While the zone is exhausted, optimized code known not to fit the
    // room left is not snapshotted, compiled and emitted in vain once per
    // budget: the request is held, as a baseline compile's is.
    if !mutator.jit.may_compile_needing(needed) {
        mutator.jit.hold_compile();
        defer_optimization(mutator, code, key);
        return;
    }
    let method = match &body.method {
        Some(method) => match method.upgrade() {
            Some(method) => Some(method),
            None => return,
        },
        None => None,
    };
    // A copy whose entered application is gone, or whose slot base no
    // index holds, is compiled as the shared body is: its own-slot
    // instructions keep their templates, which find the base and check
    // the guard, and are right for any key.
    let customization = key.and_then(|key| {
        let own_slot_base = u32::try_from(entered.upgrade()?.own_slot_base).ok()?;
        Some(optimizing::snapshot::Customization { key, own_slot_base })
    });
    let started = std::time::Instant::now();
    let snapshot = optimizing::snapshot::Snapshot::take(
        code,
        &mut mutator.heap,
        &mutator.caches,
        feedback_base,
        customization,
    );
    let mut macro_assembler = macro_assembler::MacroAssembler::new(macro_assembler::Abi::host());
    macro_assembler.set_count_back_edges(settings.counting);
    // Under the invalidate stress mode the countdown's last point leaves
    // for the safepoint that runs its round (`invalidation_due_for_stress`).
    macro_assembler
        .set_deoptimize_by_countdown(mutator.jit.deopt_stress || mutator.jit.invalidate_stress);
    macro_assembler.set_count_down_at_entry(mutator.jit.invalidate_stress);
    let role = baseline::BodyRole::of(code, method.is_some());
    let installation = optimizing::compile(
        &snapshot,
        macro_assembler,
        role,
        settings.futures_exit,
        optimizing::Speculation::at_version(code.jit.optimized_version(key)),
    )
    .and_then(|optimized| {
        install_optimized(
            &mut mutator.jit,
            code,
            method.as_ref(),
            (lean, feedback_base, settings, key),
            optimized,
        )
    });
    let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let statistics = &mut mutator.jit.statistics;
    statistics.optimize_nanoseconds = statistics.optimize_nanoseconds.saturating_add(elapsed);
    let index = match installation {
        Ok(index) => index,
        // No room is no reason to give the body up: as for a baseline
        // compile, the zone is compacted -- at this same safepoint, which
        // serves the compaction next -- and the body's budget armed
        // afresh, so that it is requested again once it has run as long.
        Err(Refusal::ZoneFull) => {
            if mutator.jit.zone_exhausted {
                mutator.jit.hold_compile();
            } else {
                mutator.jit.compaction_requested = true;
                mutator.jit.zone_exhausted = true;
            }
            defer_optimization(mutator, code, key);
            return;
        }
        Err(_) => {
            match key {
                None => code.jit.optimized_refused.set(true),
                Some(key) => code.jit.refuse_copy(key),
            }
            return;
        }
    };
    mutator.jit.bodies[lean as usize].optimized_needed = 0;
    match key {
        // An installed copy counts its traps from none, whether its
        // predecessor was retired or freed with its baseline code.
        None => {
            code.jit.optimized.set(index);
            code.jit.optimized_traps.set(0);
        }
        Some(key) => {
            code.jit.set_copy_body(key, index);
            mutator.jit.statistics.customized_copies += 1;
            mutator.jit.write_dispatching_rows(lean);
        }
    }
    mutator.jit.statistics.optimized_bodies += 1;
    // Its transfers go into the twin, which the baseline body may have.
    mutator.jit.write_transfer_words(index);
    // Every cell that called the baseline code is written afresh from its
    // entry, which names the optimized code now; and the row the closure
    // stubs enter a block through, once one was written.
    if let Some(zone) = mutator.jit.zone.as_ref() {
        let baseline_entry = zone.writer.zone().executable_address(CodeOffset(start)) as u64;
        mutator
            .caches
            .refresh_cells_entering(baseline_entry, &mutator.jit);
    }
    if let Some(id) = mutator.heap.clear_machine_entry_row(code) {
        note_block_entry(mutator, id, code);
    }
    // As for a key that has its copy (`request_copy`), the next run-out
    // is another key's.
    if key.is_some() {
        rearm_optimization_budget(mutator, code, 0);
    }
}

/// Puts off `code`'s optimization, which found no room or knows it would
/// not: a compaction asked for is passed on to the heap, and the body's
/// budget armed afresh, so that it is requested again once it has run as
/// long -- as long as the version of the copy for `key` says
/// (`optimization_budget_after`), which a full zone does not reset.
#[cfg(ns_jit)]
fn defer_optimization(
    mutator: &mut Mutator<'_>,
    code: &Rc<CompiledCode>,
    key: Option<CustomizationKey>,
) {
    if mutator.jit.compaction_requested {
        mutator.heap.request_safepoint();
    }
    rearm_optimization_budget(mutator, code, code.jit.optimized_version(key));
}

/// Installs `optimized`, the optimized code of `code` -- `method`'s body
/// when it is one's -- beside its lean baseline body at `lean` in
/// `bodies`, compiled against `feedback_base` under `settings` for `key`
/// (`None` for the body's one uncustomized copy), answering its index
/// among the installed bodies. Its call returns are recorded
/// as the baseline's are, so that a leaf and the exit's walk find the
/// send each of its calls returns to; its program-counter map has no
/// instruction's start, since no frame goes on mid-body in optimized code.
#[cfg(ns_jit)]
fn install_optimized(
    state: &mut JitState,
    code: &Rc<CompiledCode>,
    method: Option<&Rc<MixinMethod>>,
    (lean, feedback_base, settings, key): (
        u32,
        FeedbackBase,
        CompileSettings,
        Option<CustomizationKey>,
    ),
    optimized: optimizing::emit::OptimizedBody,
) -> Result<u32, Refusal> {
    let optimizing::emit::OptimizedBody {
        code: assembled,
        returns,
        words,
        twin_transfers,
        metadata: _,
        push_entry,
        ..
    } = optimized;
    let mut returns: Vec<CallReturn> = returns
        .iter()
        .map(|(label, program_counter)| CallReturn {
            machine_offset: assembled.label_offsets[label.index()]
                .expect("internal: every recorded label is bound"),
            program_counter: *program_counter,
        })
        .collect();
    returns.sort_unstable_by_key(|call| call.machine_offset);
    let body = baseline::BaselineBody {
        code: assembled,
        map: ProgramCounterMap::new(Vec::new(), returns),
        // Not asked of optimized code: its baseline code was installed,
        // so it does not exit at once either (or the JIT stress mode
        // installs every body), and it exits where the baseline does.
        exits_at_once: false,
        reifies: false,
        words,
        twin_transfers,
        budget_word: None,
        dispatching: None,
    };
    let needed = needed_bytes(&body);
    let index = install_body(
        state,
        code,
        method,
        feedback_base,
        body,
        settings,
        Some(lean),
    )
    .inspect_err(|refusal| {
        // What a compaction must free, and what the body waits for while
        // the zone stays exhausted, as for a baseline compile.
        if *refusal == Refusal::ZoneFull {
            state.wanted_bytes = state.wanted_bytes.max(needed);
            state.bodies[lean as usize].optimized_needed = needed;
        }
    })?;
    state.bodies[index as usize].push_entry = push_entry;
    state.bodies[index as usize].key = key;
    Ok(index)
}

#[cfg(not(ns_jit))]
fn compile(
    _state: &mut JitState,
    _code: &Rc<CompiledCode>,
    _feedback_base: FeedbackBase,
    _method: Option<&Rc<MixinMethod>>,
) -> Result<([CodeOffset; 2], u32), Refusal> {
    Err(Refusal::Unassembled(None))
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
/// in this interpreter's zone -- its optimized code once it has some. Out of line, so that the run loop pays only
/// `JitSlot::push_is_armed` for the JIT on a push.
#[cold]
#[inline(never)]
pub(crate) fn entered(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    code: &Rc<CompiledCode>,
    register_base: RegisterBase,
) -> Resumption {
    // An abandoned body has no machine code, and a push hands it here only
    // at the entry that revives it (`JitSlot::push_is_armed`), its others
    // counted inline.
    if code.jit.is_abandoned() && mutator.jit.enabled {
        mutator.jit.statistics.abandoned_entries_out_of_line += 1;
        mutator.jit.note_abandoned_entry(code);
        after_compile_trigger(mutator);
        return Resumption::Interpreted {
            activation,
            program_counter: 0,
        };
    }
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
        after_compile_trigger(mutator);
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
    // A push enters the body's optimized code for the frame's key once it
    // has some, unless the frame is reified already, which only the twin,
    // baseline code, keeps (`entry_of_frame`).
    let key = activation_key(&mutator.heap, code, activation);
    let optimized = mutator
        .jit
        .optimized_push_entry(code, feedback_base, key)
        .filter(|_| mutator.heap.activation_reified(activation).is_none());
    match optimized.or_else(|| entry_of_frame(mutator, activation, code, feedback_base, 0)) {
        Some(entry) => run_machine_code(mutator, activation, register_base, entry),
        None => Resumption::Interpreted {
            activation,
            program_counter: 0,
        },
    }
}

/// Where the run loop enters `code`'s machine code for `activation`, with
/// `feedback_base`, at the instruction at `program_counter`: in the lean
/// body for a frame whose reified word is nil, and in the twin for one the
/// interpreter reified -- a frame an exit left, whose sender walk, or the
/// `MakeClosure` the exit handed over, reified it -- compiled now if it is
/// not yet, so that its sends store their program counters and its return
/// divorces it, as the interpreter's pop would (`docs/design/execution.md`,
/// "A frame reifies only in its body's twin"). `None` for a body without
/// machine code here, and for a twin the zone has no room for.
fn entry_of_frame(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    code: &CompiledCode,
    feedback_base: FeedbackBase,
    program_counter: u32,
) -> Option<CodeOffset> {
    if mutator.heap.activation_reified(activation).is_none() {
        return mutator
            .jit
            .interpreter_entry(code, feedback_base, program_counter);
    }
    let entry = twin_entry(&mut mutator.jit, code, feedback_base, program_counter);
    after_compile_trigger(mutator);
    #[cfg(ns_jit)]
    if entry.is_none() && code.jit.twin_refused.get() {
        let code = mutator.heap.activation_code(activation);
        refuse_for_its_twin(mutator, &code);
    }
    entry
}

/// Where `code`'s twin, compiled against `feedback_base`, starts the
/// instruction at `program_counter`, the twin compiled if it is not yet.
#[cfg(ns_jit)]
fn twin_entry(
    state: &mut JitState,
    code: &CompiledCode,
    feedback_base: FeedbackBase,
    program_counter: u32,
) -> Option<CodeOffset> {
    let lean = u32::try_from(state.named_body(code)?).ok()?;
    if !state.is_entered_here(code) || state.bodies[lean as usize].feedback_base != feedback_base {
        return None;
    }
    let twin = compile_twin(state, lean).ok()?;
    let twin = &state.bodies[twin as usize];
    let offset = twin.map.machine_offset(program_counter)?;
    Some(CodeOffset(twin.start + offset))
}

#[cfg(not(ns_jit))]
fn twin_entry(
    _state: &mut JitState,
    _code: &CompiledCode,
    _feedback_base: FeedbackBase,
    _program_counter: u32,
) -> Option<CodeOffset> {
    None
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
    match entry_of_frame(mutator, activation, code, feedback_base, program_counter) {
        Some(entry) if native_stack_has_room() => {
            mutator.jit.statistics.resumed_entries += 1;
            if mutator.jit.has_optimized_code(code) {
                mutator.jit.statistics.optimized_bodies_resumed += 1;
            }
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

/// An interpreted backward jump of `activation`, whose body `code` has an
/// armed slot (`JitSlot::back_edge_is_armed`), to `target`, the head of
/// the loop it closes: answers whether the frame switches into machine
/// code there, on-stack replacement (`docs/design/execution.md`, "Between
/// the tiers"), so that the run loop ends its stretch for `at_back_edge`
/// only when the switch happens. A body without machine code in this zone
/// counts the jump, and at its `JitState::compile_at_back_jump`th is
/// compiled as an entry compiles it; one compiled into another zone is
/// compiled into this one at once, as `note_entry` compiles it at its next
/// entry, unless the exhausted zone would hold the compile, when it counts
/// its jumps as a body not yet compiled does. A body with machine code switches
/// when the frame can enter it at `target` -- its feedback base the
/// body's, the twin compiled for a reified frame, room on the native
/// stack (`entry_of_frame`); when it cannot, the body waits as many back
/// jumps again before it tries, so that neither a frame that never can
/// nor a twin an exhausted zone holds costs every turn a try. Out of
/// line, so that the interpreter's backward jump pays only the slot's
/// test for the JIT inline.
#[cold]
#[inline(never)]
pub(crate) fn back_edge_ready(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    code: &Rc<CompiledCode>,
    target: usize,
) -> bool {
    let slot = &code.jit;
    let Ok(target) = u32::try_from(target) else {
        unreachable!("internal: a program counter fits 32 bits");
    };
    let feedback_base = mutator.heap.activation_feedback_base(activation);
    if mutator.jit.is_entered_here(code) {
        // A body whose frame could not enter at its last try waits.
        if slot.back_jumps.get() > 0 && !slot.count_back_jump(mutator.jit.compile_at_back_jump) {
            return false;
        }
    } else {
        // Refused as `note_entry` refuses a body entered while the JIT is
        // off, and as `entered` refuses a top-level activation's, whose
        // frame machine code would treat as a factory's.
        if !mutator.jit.enabled
            || matches!(
                mutator.heap.activation_kind(activation),
                crate::types::ActivationKind::TopLevel
            )
        {
            slot.refuse();
            mutator.jit.statistics.refused_bodies += 1;
            return false;
        }
        // A body compiled into another zone is compiled into this one at
        // once, as `note_entry` compiles it at its next entry: it came due
        // already, where it was compiled first. Only when the zone has
        // room: a compile the exhausted zone holds waits its back jumps,
        // so that a held body costs a try, and a held compile towards
        // the next compaction, once per wait and not every turn.
        if slot.state.get() == COMPILED && mutator.jit.may_compile_now(slot) {
            slot.back_jumps.set(0);
        } else if !slot.count_back_jump(mutator.jit.compile_at_back_jump) {
            return false;
        }
        if mutator.jit.may_compile_now(slot) {
            let method = mutator.heap.activation_method(activation);
            compile_into_slot(&mut mutator.jit, code, feedback_base, method.as_ref());
        } else {
            mutator.jit.hold_compile();
        }
        after_compile_trigger(mutator);
        if !mutator.jit.is_entered_here(code) {
            return false;
        }
        mutator.jit.statistics.back_edge_compiles += 1;
        // The row the closure stubs enter a block through, which only a
        // push writes otherwise (`note_block_entry`).
        #[cfg(ns_jit)]
        if code.kind == crate::bytecode::CodeKind::Block {
            let id = mutator.heap.activation_code_id(activation);
            note_block_entry(mutator, id, code);
        }
    }
    // Compiled frames keep their return addresses on the native stack.
    let ready = native_stack_has_room()
        && entry_of_frame(mutator, activation, code, feedback_base, target).is_some();
    if !ready && mutator.jit.is_entered_here(code) {
        slot.back_jumps.set(1);
    }
    ready
}

/// The switch `back_edge_ready` answered for: runs `activation`'s
/// machine code from `target`, the loop's head, through its
/// program-counter map -- the twin's for a reified frame, as for a frame
/// returned to (`entry_of_frame`) -- since at a back edge baseline code
/// holds every value in the frame's words, which are already machine
/// code's. Counts nothing and compiles nothing but a twin. `None` when
/// the frame goes on interpreted at `target`: its body has no machine
/// code here it can enter.
#[cold]
#[inline(never)]
pub(crate) fn at_back_edge(
    mutator: &mut Mutator<'_>,
    activation: ActivationReference,
    code: &Rc<CompiledCode>,
    register_base: RegisterBase,
    target: u32,
) -> Option<Resumption> {
    // `run_machine_code` would answer program counter 0 without room.
    if !mutator.jit.is_entered_here(code) || !native_stack_has_room() {
        return None;
    }
    // `back_edge_ready` refuses a top-level activation's body before it
    // compiles it, and a body compiled at a push stays one no top-level
    // activation runs, as at `entered`.
    debug_assert!(
        !matches!(
            mutator.heap.activation_kind(activation),
            crate::types::ActivationKind::TopLevel
        ),
        "internal: machine code is entered for a top-level activation"
    );
    let feedback_base = mutator.heap.activation_feedback_base(activation);
    let entry = entry_of_frame(mutator, activation, code, feedback_base, target)?;
    mutator.jit.statistics.back_edge_switches += 1;
    Some(run_machine_code(mutator, activation, register_base, entry))
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
    mutator.jit.walked_optimized = false;
    mutator.jit.refused_for_an_uncompiled_callee = false;
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
    let (innermost, optimized) = match mutator.jit.take_walked() {
        Some(walked) => walked,
        // The entry's own frame exited, in the code the exit names.
        None => (activation, mutator.jit.exited_optimized(&block)),
    };
    resumption_after_exit(mutator, &block, kind, innermost, optimized)
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
    // A body with a customized copy is called at its dispatching entry,
    // whose rows find the receiver's copy in machine code, as a `super` or
    // private lexical site's cell does: the key's lookups on this side
    // took 1.4% of CD's samples, on its interpreted sends alone. A body
    // with more copies than its rows hold takes the lookup, which reaches
    // every copy, and a body with none enters its code without a key.
    let machine_entry = called_dispatching_entry(&mutator.jit, code, feedback_base).or_else(|| {
        let key = entry_key(&mutator.heap, code, receiver, Some(defining_class));
        machine_entry_address(&mutator.jit, code, feedback_base, key)
    });
    let Some(machine_entry) = machine_entry else {
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
    let Some(machine_entry) = machine_entry_address(&mutator.jit, code, feedback_base, None) else {
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
    mutator.jit.walked_optimized = false;
    mutator.jit.refused_for_an_uncompiled_callee = false;
    let kind = enter_block(mutator, &zone_code, call_stub, &block);
    if kind == ExitKind::Returned as u64 {
        // No exit took the seed.
        mutator.jit.walk.clear();
        return Called::Returned {
            value: block.get(BlockWord::ExitProgramCounter.index()),
        };
    }
    let Some((innermost, optimized)) = mutator.jit.take_walked() else {
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
        mutator.jit.statistics.pushes_refused += 1;
        return Called::Refused;
    }
    Called::Stopped(resumption_after_exit(
        mutator, &block, kind, innermost, optimized,
    ))
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
    unreachable!("internal: no body has machine code without a JIT")
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
    unreachable!("internal: no body has machine code without a JIT")
}

/// Records where the block body `code`, whose id is `id`, enters machine
/// code -- its optimized code once it has some (`JitState::entered_body`)
/// -- in the machine entry table the closure stubs read
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
/// switch of the JIT on or off, the install of its optimized code, which
/// writes it afresh -- so a row the table records is current, and a push of a block whose row is
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
    let Some(body) = mutator.jit.entered_body(code, None) else {
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
        block.set(BlockWord::DispatchedCalls.index(), 0);
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
        mutator.jit.statistics.dispatched_calls += block.get(BlockWord::DispatchedCalls.index());
    }
    kind
}

/// Where the run loop goes on after machine code exited with `kind`, no
/// return, as `block` describes the exit, `innermost` the innermost frame
/// the exit materialized or the entry's, `optimized` whether it stood in
/// optimized code: the exit counted, a body that has spent its budget
/// abandoned, and the innermost frame interpreted from here.
#[cfg(ns_jit)]
fn resumption_after_exit(
    mutator: &mut Mutator<'_>,
    block: &EntryBlock,
    kind: u64,
    innermost: ActivationReference,
    optimized: bool,
) -> Resumption {
    use stubs::MachineExit;

    let exit = stubs::decode_exit(block, kind);
    mutator.jit.statistics.exits += 1;
    // The deopt stress mode's forced exits count against no budget, as
    // the exit stress mode's forced polls do not: they say nothing of the
    // code, and counted they would abandon every optimized body within a
    // few dozen of its points, under a mode that optimizes it again at
    // once.
    let forced = optimized && mutator.jit.take_forced_exit(&mut mutator.heap);
    if optimized {
        mutator.jit.statistics.optimized_exits += 1;
    }
    // Counted on every exit, not only while counting, since the abandonment
    // and the tests read it: a match on the exit's slot, which decodes
    // nothing (`CompiledCode::is_send`), on a path that has just left
    // machine code.
    let at_a_send = match exit {
        MachineExit::PushRefused { .. } => true,
        MachineExit::Reexecute { program_counter } => mutator
            .heap
            .activation_code(innermost)
            .is_send(program_counter as usize),
        _ => false,
    };
    if at_a_send {
        mutator.jit.statistics.send_exits += 1;
    }
    // A push the callee's prologue refused at the depth limit word while a
    // real safepoint request stands was refused for the request, which
    // lowered the word to 0 -- often the optimization request a freshly
    // compiled callee's first spend made -- whatever the depth: like a
    // forced exit it says nothing of the caller's code, and counted it
    // would abandon a hot caller of callees optimized one after another.
    // The interpreter's push, at the safepoint, decides the depth limit.
    let refused_for_a_request = matches!(exit, MachineExit::PushRefused { at_the_bound: true })
        && mutator.heap.real_safepoint_requested();
    // A call the miss leaf refused because its callee has no machine code
    // yet, while the zone has room to compile it, says nothing of the
    // caller's code either: the interpreter's push counts the callee's
    // entry, and an entry to come compiles it (`note_call_refused`).
    // Counted, a caller whose callees a compaction frees -- each counting
    // its entries again -- would spend a refusal on every callee every
    // compaction freed, and be abandoned for what the zone did.
    let refused_for_an_uncompiled_callee =
        std::mem::take(&mut mutator.jit.refused_for_an_uncompiled_callee)
            && matches!(
                exit,
                MachineExit::PushRefused {
                    at_the_bound: false
                }
            );
    // The stress mode enters every body at every push, however often it
    // leaves.
    if matches!(
        exit,
        MachineExit::Reexecute { .. } | MachineExit::PushRefused { .. }
    ) && !mutator.jit.install_bodies_exiting_at_once
        && !forced
        && !refused_for_a_request
        && !refused_for_an_uncompiled_callee
    {
        let code = mutator.heap.activation_code(innermost);
        // The copy the frame ran is the one its key names: a copy is
        // entered only where its key holds.
        let key = optimized
            .then(|| activation_key(&mutator.heap, &code, innermost))
            .flatten();
        count_exit(mutator, &code, optimized, key);
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
        // A trap runs its instruction again, as a template's exit does,
        // which widens the feedback the speculation failed on.
        MachineExit::Trap {
            program_counter,
            site,
        } => {
            debug_assert!(optimized, "internal: only optimized code traps");
            count_trap(mutator, innermost, site);
            Resumption::Interpreted {
                activation: innermost,
                program_counter,
            }
        }
        // The innermost frame is the caller, stopped at the send the walk
        // wrote as its program counter: it runs the send again.
        MachineExit::PushRefused { .. } => {
            mutator.jit.statistics.pushes_refused += 1;
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
/// body is settled abandoned (`FIRST_ABANDONED`), so that no push and no frame returned to
/// enters it in machine code again until it is revived -- its exits decay
/// (`JitState::decay_exits`) or its interpreted entries add up
/// (`JitState::note_abandoned_entry`) -- and
/// every cell that names its machine entry is written afresh from its
/// entry, which now names the stub that refuses the call. Its machine code
/// -- baseline and optimized code alike -- is freed at once
/// (`JitState::free_body`), and its bytes reclaimed by the
/// next compaction, so that a revived body is compiled anew: abandonment
/// runs only between entries (`resumption_after_exit`), where no return
/// address into the zone exists, and a frame an exit left stopped in the
/// code resumes interpreted (`resumed`), as every frame stopped in freed
/// code does.
#[cfg(ns_jit)]
fn abandon(mutator: &mut Mutator<'_>, code: &Rc<CompiledCode>) {
    let entries = mutator.jit.free_entered_code(code);
    code.jit.abandon(mutator.jit.revival_base);
    mutator.jit.abandoned.push(Rc::downgrade(code));
    mutator.jit.statistics.abandoned_bodies += 1;
    // A block's closure stubs call it no more either.
    mutator.heap.forget_machine_entry(code);
    mutator
        .caches
        .refresh_cells_entering_any(&entries, &mutator.jit);
}

/// Counts an exit that handed `code`'s frame to the interpreter, which
/// stood in `code`'s optimized code for `key` if `optimized`, and gives
/// up what
/// keeps exiting: a body whose twin the compiler refused is refused for
/// good, whichever code exited, since every copy exits where it makes a
/// closure; an exit of optimized code counts against the optimized
/// code's own `EXIT_BUDGET`, which, spent, abandons it alone
/// (`abandon_optimized`), its baseline code staying installed; an exit of
/// baseline code against the slot's, which, spent, abandons the body --
/// baseline and optimized code alike (`abandon`). Between entries only.
#[cfg(ns_jit)]
fn count_exit(
    mutator: &mut Mutator<'_>,
    code: &Rc<CompiledCode>,
    optimized: bool,
    key: Option<CustomizationKey>,
) {
    if code.jit.twin_refused.get() {
        refuse_for_its_twin(mutator, code);
        return;
    }
    if optimized {
        if mutator.jit.note_optimized_exit(code, key) {
            abandon_optimized(mutator, code, key);
        }
        return;
    }
    if code.jit.note_exit() {
        abandon(mutator, code);
    }
}

/// Abandons `code`'s optimized code for `key`, which has spent its
/// `EXIT_BUDGET` -- the templates' exits, never its traps: retired as a
/// copy made not entrant is (`retire_optimized`), since an abandonment is
/// a version too, so that code with no trap that keeps exiting backs off
/// as a trap's recompile does, and is left in baseline code for good past
/// its last version, as HotSpot stays interpreted past
/// `PerMethodRecompilationCutoff` (`docs/design/execution.md`, "Speculation
/// and its failure"). Its exits and its usage stay what they were.
#[cfg(ns_jit)]
fn abandon_optimized(
    mutator: &mut Mutator<'_>,
    code: &Rc<CompiledCode>,
    key: Option<CustomizationKey>,
) {
    if retire_optimized(mutator, code, key) {
        mutator.jit.statistics.abandoned_optimized_bodies += 1;
    }
}

/// Counts a trap of the optimized code the innermost frame of an exit,
/// `innermost`, ran, at a speculation resting on `site`: the site's
/// trapped bit is set, so that the next compile gives it its slow path,
/// and the copy is made not entrant (`retire_optimized`) when the site
/// had trapped before -- the same speculation failed again, in code that
/// will go on failing it -- or the copy's traps reach `TRAP_LIMIT`.
/// Counted against no exit budget: a trap says the feedback has widened,
/// not that the code exits where it must. Between entries only, as every
/// retirement is.
#[cfg(ns_jit)]
fn count_trap(mutator: &mut Mutator<'_>, innermost: ActivationReference, site: u32) {
    mutator.jit.statistics.traps += 1;
    let code = mutator.heap.activation_code(innermost);
    // The copy the frame ran is the one its key names: a copy is entered
    // only where its key holds.
    let key = activation_key(&mutator.heap, &code, innermost);
    let Some(body) = mutator.jit.optimized_for(&code, key) else {
        return;
    };
    let (feedback_base, copy) = (body.feedback_base, body.key);
    // A site past the family runs uncached and records nothing, as one of
    // a family without a range does, and its bit would be a neighbouring
    // family's: it reads as trapped before, as the snapshot reads it.
    let trapped_before = site >= code.family_size || mutator.caches.note_trap(feedback_base, site);
    let traps = code.jit.count_trap(copy);
    if (trapped_before || traps >= TRAP_LIMIT) && retire_optimized(mutator, &code, copy) {
        mutator.jit.statistics.made_not_entrant += 1;
    }
}

/// Retires `code`'s optimized code for `key` (`JitState::optimized_for`),
/// made not entrant for its traps or abandoned for its exits, answering
/// whether it had some: it alone is freed (`JitState::free_optimized`) --
/// the body's other copies staying -- and the body goes on in its baseline
/// code, which every cell, dispatching row and machine entry row that
/// entered the copy is written afresh to enter, as the copy's install
/// wrote them to enter it. Its version is raised (`JitSlot::raise_version`)
/// and the body's budget armed at the new version's wait
/// (`optimization_budget_after`), so that a copy that keeps failing is
/// compiled again ever more rarely, as Strongtalk backs off a
/// recompilation, and past `LAST_VERSION` without a trap; one that fails
/// past it is refused for good, its key never requested again, and the
/// budget left as it is for the body's other keys -- and re-armed for
/// them whenever a frame of the refused pair runs it out
/// (`rearm_for_other_pairs`). Between entries only,
/// where no return address into the zone exists; a frame an exit left
/// stopped in the copy goes on in baseline code, as every frame returned
/// to does.
#[cfg(ns_jit)]
fn retire_optimized(
    mutator: &mut Mutator<'_>,
    code: &Rc<CompiledCode>,
    key: Option<CustomizationKey>,
) -> bool {
    let mut entries = Vec::new();
    let Some(copy) = unlink_optimized(mutator, code, key, &mut entries) else {
        return false;
    };
    refresh_cells_entering(mutator, entries);
    match code.jit.raise_version(copy) {
        Some(version) => rearm_optimization_budget(mutator, code, version),
        None => mutator.jit.statistics.refused_for_good += 1,
    }
    true
}

/// Frees `code`'s optimized code for `key` (`JitState::free_optimized`)
/// and writes afresh the machine entry row that named it, answering the
/// key it was compiled for, `None` for the uncustomized copy, and adding
/// to `entries` the addresses a cell may name of it -- its start, and the
/// body's dispatching entry, which a `super` or private lexical cell
/// names only while the body has a copy -- for the caller to write the
/// cells naming them afresh, once for every copy it frees
/// (`refresh_cells_entering`). Its dispatching rows are written afresh by
/// the free itself. Between entries only.
#[cfg(ns_jit)]
fn unlink_optimized(
    mutator: &mut Mutator<'_>,
    code: &Rc<CompiledCode>,
    key: Option<CustomizationKey>,
    entries: &mut Vec<u64>,
) -> Option<Option<CustomizationKey>> {
    let (start, copy) = mutator.jit.free_optimized(code, key)?;
    if let Some(zone) = mutator.jit.zone.as_ref() {
        entries.push(zone.writer.zone().executable_address(CodeOffset(start)) as u64);
    }
    entries.extend(mutator.jit.dispatching_entry_address(code));
    if let Some(id) = mutator.heap.clear_machine_entry_row(code) {
        note_block_entry(mutator, id, code);
    }
    Some(copy)
}

/// Writes afresh from its entry every cell that names one of `entries`
/// (`InlineCaches::refresh_cells_entering_any`): one walk of the cells for
/// all the copies an invalidation freed.
#[cfg(ns_jit)]
fn refresh_cells_entering(mutator: &mut Mutator<'_>, mut entries: Vec<u64>) {
    if entries.is_empty() {
        return;
    }
    entries.sort_unstable();
    entries.dedup();
    mutator
        .caches
        .refresh_cells_entering_any(&entries, &mutator.jit);
}

/// Frees exactly `code`'s optimized code for `key` -- the key's copy, or
/// for `None` the body's one uncustomized copy, never the one an
/// activation of the key would run in its place -- as an invalidation
/// does, counting it (`JitStatistics::invalidated_copies`), and answers
/// whether it had some. No version is raised and no trap counted: what
/// the copy was compiled against changed, which says nothing of its
/// speculations (`docs/design/execution.md`, "Speculation", "Invalidation").
#[cfg(ns_jit)]
fn invalidate_copy(
    mutator: &mut Mutator<'_>,
    code: &Rc<CompiledCode>,
    key: Option<CustomizationKey>,
    entries: &mut Vec<u64>,
) -> bool {
    let exists = match key {
        Some(key) => mutator.jit.copy_of(code, key).is_some(),
        None => mutator.jit.optimized(code).is_some(),
    };
    if !exists || unlink_optimized(mutator, code, key, entries).is_none() {
        return false;
    }
    mutator.jit.statistics.invalidated_copies += 1;
    true
}

/// Frees every customized copy of `code` whose key `stale` says is stale,
/// and the body's one uncustomized copy too when `uncustomized` is set, adding
/// the addresses cells may name of them to `entries`; then, if any was
/// freed, arms the body's budget afresh at the longest wait the freed
/// copies' versions earned (`optimization_budget_after`), so that the next
/// copy waits for feedback gathered since -- as long as a failure of the
/// freed one would have it wait, no longer, since an invalidation is no
/// failure.
#[cfg(ns_jit)]
fn invalidate_copies_of(
    mutator: &mut Mutator<'_>,
    code: &Rc<CompiledCode>,
    stale: &dyn Fn(CustomizationKey) -> bool,
    uncustomized: bool,
    entries: &mut Vec<u64>,
) {
    let keys: Vec<(CustomizationKey, u8)> = code
        .jit
        .copies
        .borrow()
        .iter()
        .filter(|copy| stale(copy.key))
        .map(|copy| (copy.key, copy.version))
        .collect();
    let mut wait: Option<u8> = None;
    let mut note = |version: u8| wait = Some(wait.map_or(version, |longest| longest.max(version)));
    if uncustomized && invalidate_copy(mutator, code, None, entries) {
        note(code.jit.optimized_version(None));
    }
    for (key, version) in keys {
        if invalidate_copy(mutator, code, Some(key), entries) {
            note(version);
        }
    }
    if let Some(version) = wait {
        rearm_optimization_budget(mutator, code, version);
    }
}

/// Frees every customized copy whose key names one of `rebuilt` -- the
/// instances ids of the applications an install replaced
/// (`Mutator::install_edit`) -- as its receiver's or its entered
/// application: each was compiled against a layout, a lookup or an outer
/// structure the install changed. Every other copy stays, and so does
/// each key's row, whose ids no receiver carries any more once the
/// install restamped or forwarded every instance; a full collection
/// drops the rows once it reclaims the ids (`forget_freed_copies`).
/// Freed at once, with their cells, dispatching rows and machine entry
/// rows written afresh: an install runs after an exit, where no return
/// address into the zone exists and every frame goes on in its baseline
/// code (`docs/design/execution.md`, "Speculation", "Invalidation"). No
/// version is raised (`invalidate_copy`).
#[cfg(ns_jit)]
pub(crate) fn invalidate_applications(mutator: &mut Mutator<'_>, rebuilt: &[DispatchId]) {
    if rebuilt.is_empty() {
        return;
    }
    let rebuilt: std::collections::HashSet<DispatchId, crate::hash::IdentityBuildHasher> =
        rebuilt.iter().copied().collect();
    let stale =
        |key: CustomizationKey| rebuilt.contains(&key.receiver) || rebuilt.contains(&key.entered);
    // A request still pending would compile a copy for the old layout.
    mutator
        .jit
        .optimization_requests
        .retain(|request| request.key.is_none_or(|key| !stale(key)));
    let mut entries = Vec::new();
    for code in mutator.jit.customized_codes() {
        invalidate_copies_of(mutator, &code, &stale, false, &mut entries);
    }
    refresh_cells_entering(mutator, entries);
}

/// Frees every optimized copy there is, customized or not: what a bulk
/// forward (`elementsOf:forwardIdentityToElementsOf:ifFail:`) does, since
/// a forwarded object may be the receiver a copy was verified for or
/// reached through any of its slots; the invalidate stress mode's rounds
/// free the same, but for the bodies backing off
/// (`invalidate_optimized_for_stress`). Freed at once, as
/// `invalidate_applications` frees: the forward is a mirror leaf run
/// after an exit.
/// No version is raised (`invalidate_copy`).
#[cfg(ns_jit)]
pub(crate) fn invalidate_all_optimized(mutator: &mut Mutator<'_>) {
    let mut entries = Vec::new();
    for code in mutator.jit.codes_with_optimized_bodies() {
        invalidate_copies_of(mutator, &code, &|_| true, true, &mut entries);
    }
    refresh_cells_entering(mutator, entries);
}

/// The invalidate stress mode's round (`NS_NEXT_STRESS_INVALIDATE`), at
/// the run loop's safepoint once optimized code has spent the countdown
/// (`JitState::invalidation_due_for_stress`), before what was requested
/// since the last one is installed: frees every body's copies as
/// `invalidate_all_optimized` does, but those of a body that sits this
/// round out (`StressBackOff`), and arms the countdown afresh. Nothing
/// outside the mode, or before the countdown is spent.
#[cfg(ns_jit)]
pub(crate) fn invalidate_optimized_for_stress(mutator: &mut Mutator<'_>) {
    if !mutator.jit.invalidate_stress() || !mutator.jit.invalidation_due_for_stress() {
        return;
    }
    let mut entries = Vec::new();
    for code in mutator.jit.codes_with_optimized_bodies() {
        if mutator.jit.stress_round_frees(&code) {
            invalidate_copies_of(mutator, &code, &|_| true, true, &mut entries);
        }
    }
    refresh_cells_entering(mutator, entries);
    mutator
        .jit
        .stress_back_offs
        .retain(|_, back_off| back_off.code.strong_count() > 0);
    mutator.jit.arm_invalidation_count();
}

impl JitState {
    /// Every live body with optimized code installed, each once.
    #[cfg(ns_jit)]
    fn codes_with_optimized_bodies(&self) -> Vec<Rc<CompiledCode>> {
        let mut codes: Vec<Rc<CompiledCode>> = Vec::new();
        for body in &self.bodies {
            if body.free || body.tier != Tier::Optimized {
                continue;
            }
            if let Some(code) = body.code.upgrade() {
                if !codes.iter().any(|known| Rc::ptr_eq(known, &code)) {
                    codes.push(code);
                }
            }
        }
        codes
    }
}

/// Drops every key's row whose receiver's or entered application's id a
/// full collection reclaimed (`Mutator::reclaim_ids`), with its copy and
/// every pending request for it: the id is minted again for another
/// application, of another layout, which must meet neither the copy nor
/// what the row says of the dead one -- a refusal that would block the
/// new key for good, a version that would hold back its first copy, the
/// level and the application word its cells and dispatching rows would
/// match it by. Run in the collection, between marking and compaction,
/// where nothing runs; it allocates nothing and moves nothing. No version
/// is raised and no budget armed: the row it would be raised in is gone.
#[cfg(ns_jit)]
pub(crate) fn forget_freed_copies(
    mutator: &mut Mutator<'_>,
    freed: &crate::heap::reclaim::FreedIds,
) {
    if freed.guards.is_empty() {
        return;
    }
    let stale = |key: CustomizationKey| {
        freed.guards.contains(&key.receiver) || freed.guards.contains(&key.entered)
    };
    mutator
        .jit
        .optimization_requests
        .retain(|request| request.key.is_none_or(|key| !stale(key)));
    let mut entries = Vec::new();
    for code in mutator.jit.customized_codes() {
        let keys: Vec<CustomizationKey> = code
            .jit
            .copies
            .borrow()
            .iter()
            .map(|copy| copy.key)
            .filter(|key| stale(*key))
            .collect();
        for key in keys {
            invalidate_copy(mutator, &code, Some(key), &mut entries);
        }
        code.jit.copies.borrow_mut().retain(|copy| !stale(copy.key));
    }
    mutator
        .jit
        .customized
        .retain(|code| code.upgrade().is_some_and(|code| code.jit.has_copies()));
    refresh_cells_entering(mutator, entries);
}

/// Without a JIT there is no optimized code to invalidate.
#[cfg(not(ns_jit))]
pub(crate) fn invalidate_applications(_mutator: &mut Mutator<'_>, _rebuilt: &[DispatchId]) {}

/// Without a JIT there is no optimized code to invalidate.
#[cfg(not(ns_jit))]
pub(crate) fn invalidate_all_optimized(_mutator: &mut Mutator<'_>) {}

/// Without a JIT there is no optimized code to invalidate.
#[cfg(not(ns_jit))]
pub(crate) fn invalidate_optimized_for_stress(_mutator: &mut Mutator<'_>) {}

/// Without a JIT no body has a key's row.
#[cfg(not(ns_jit))]
pub(crate) fn forget_freed_copies(
    _mutator: &mut Mutator<'_>,
    _freed: &crate::heap::reclaim::FreedIds,
) {
}

/// The budget a body is armed with for a copy at `version`: `budget` times
/// 4 to the version, saturating, Strongtalk's wait before a recompile
/// (`docs/design/execution.md`, "Speculation", "Versions and the refusal
/// for good"), so that a
/// copy compiled at its first run-out waits the plain budget and each
/// failure quadruples the next wait.
fn optimization_budget_after(budget: i64, version: u8) -> i64 {
    budget.saturating_mul(4_i64.saturating_pow(u32::from(version)))
}

impl JitState {
    /// Whether the innermost frame of an exit that walked no frame -- the
    /// entry's own -- stood in optimized code: the code the exit names
    /// (`stubs::BlockWord::ExitCodeAddress`).
    #[cfg(ns_jit)]
    fn exited_optimized(&self, block: &EntryBlock) -> bool {
        let address = block.get(stubs::BlockWord::ExitCodeAddress.index());
        usize::try_from(address)
            .ok()
            .and_then(|address| self.body_of(address))
            .is_some_and(|installed| installed.optimized)
    }

    /// Counts an exit of `code`'s optimized code for `key` against its own
    /// budget, answering whether it has just spent its `EXIT_BUDGET`.
    #[cfg(ns_jit)]
    fn note_optimized_exit(&self, code: &CompiledCode, key: Option<CustomizationKey>) -> bool {
        let Some(body) = self.optimized_for(code, key) else {
            return false;
        };
        let exits = body.exits.get().saturating_add(1);
        body.exits.set(exits);
        exits >= EXIT_BUDGET
    }

    /// Frees `code`'s optimized code for `key` alone (`optimized_for`), if
    /// it has some here, and forgets it in the slot -- the key's row
    /// staying -- answering its start for the cells that named it and the
    /// key it was compiled for, `None` for the uncustomized copy: its
    /// baseline code, its other copies, its twin and their exits stay.
    #[cfg(ns_jit)]
    fn free_optimized(
        &mut self,
        code: &CompiledCode,
        key: Option<CustomizationKey>,
    ) -> Option<(u32, Option<CustomizationKey>)> {
        let body = self.optimized_for(code, key)?;
        let (start, copy) = (body.start, body.key);
        let index = match copy {
            Some(copy) => code.jit.copy_row(copy).map(|row| row.body),
            None => Some(code.jit.optimized.get()),
        };
        let Some(index) = index else {
            unreachable!("internal: a live copy has its row");
        };
        self.free_one(index);
        match copy {
            Some(copy) => code.jit.set_copy_body(copy, NO_BODY),
            None => code.jit.optimized.set(NO_BODY),
        }
        Some((start, copy))
    }

    /// Frees `code`'s baseline code and the optimized code beside it -- its
    /// one copy and every customized one -- if it has machine code here
    /// (`JitState::free_body`), answering the addresses a cell may name of
    /// them -- each copy's start, sorted -- for the caller to write afresh
    /// once the slot says what it says now.
    #[cfg(ns_jit)]
    fn free_entered_code(&mut self, code: &CompiledCode) -> Vec<u64> {
        let Some(lean) = self.installed(code).map(|_| code.jit.map.get()) else {
            return Vec::new();
        };
        let dispatching = self.bodies[lean as usize]
            .dispatching
            .map(|dispatching| self.bodies[lean as usize].start + dispatching.entry);
        let starts: Vec<u32> = std::iter::once(lean)
            .chain(self.optimized_copies_of(lean))
            .map(|index| self.bodies[index as usize].start)
            .chain(dispatching)
            .collect();
        self.free_body(code.jit.map.get());
        let Some(zone) = self.zone.as_ref() else {
            return Vec::new();
        };
        let mut entries: Vec<u64> = starts
            .into_iter()
            .map(|start| zone.writer.zone().executable_address(CodeOffset(start)) as u64)
            .collect();
        entries.sort_unstable();
        entries
    }
}

/// Refuses `code` for good once the compiler refused its twin for a reason
/// no compaction removes (`JitSlot::twin_refused`): every activation that
/// reifies its frame in machine code would exit, so the body stays
/// interpreted, as a reifying body the compiler refused did before twins.
/// Its machine code and its twin's are freed, and every cell and machine
/// entry row that named it written afresh, as an abandonment does
/// (`abandon`); between entries only.
#[cfg(ns_jit)]
fn refuse_for_its_twin(mutator: &mut Mutator<'_>, code: &Rc<CompiledCode>) {
    let entries = mutator.jit.free_entered_code(code);
    if code.jit.state.get() != REFUSED {
        code.jit.refuse();
        mutator.jit.statistics.refused_bodies += 1;
    }
    mutator.heap.forget_machine_entry(code);
    mutator
        .caches
        .refresh_cells_entering_any(&entries, &mutator.jit);
}

#[cfg(not(ns_jit))]
fn run_machine_code(
    _mutator: &mut Mutator<'_>,
    _activation: ActivationReference,
    _register_base: RegisterBase,
    _entry: CodeOffset,
) -> Resumption {
    unreachable!("internal: no body has machine code without a JIT")
}

#[cfg(test)]
mod tests {
    use super::{CallReturn, JitSlot, ProgramCounterMap};

    #[test]
    fn the_jit_is_available_on_its_architectures() {
        let has_a_back_end = cfg!(target_arch = "x86_64")
            || (cfg!(target_arch = "aarch64")
                && (cfg!(target_os = "linux") || cfg!(target_os = "macos")));
        assert!(!has_a_back_end || super::available());
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
    fn the_entries_a_revival_waits_for_double_up_to_their_cap() {
        use super::{revival_entries, REVIVAL_ENTRIES};
        assert_eq!(revival_entries(REVIVAL_ENTRIES, 1), 4096);
        assert_eq!(revival_entries(REVIVAL_ENTRIES, 2), 8192);
        assert_eq!(revival_entries(REVIVAL_ENTRIES, 5), 65_536);
        assert_eq!(revival_entries(REVIVAL_ENTRIES, 6), 65_536, "capped");
        assert_eq!(revival_entries(REVIVAL_ENTRIES, u32::MAX), 65_536);
        assert_eq!(
            revival_entries(u32::MAX, 5),
            65_536,
            "never past the states a slot has"
        );
        assert_eq!(revival_entries(0, 1), 1, "at least one");
    }

    #[test]
    fn a_decay_halves_the_abandonments() {
        let slot = JitSlot::default();
        for _ in 0..4 {
            slot.abandon(super::REVIVAL_ENTRIES);
        }
        assert_eq!(slot.entries_before_revival(), 32_768);
        slot.decay_exits();
        slot.abandon(super::REVIVAL_ENTRIES);
        assert_eq!(
            slot.entries_before_revival(),
            16_384,
            "the third abandonment's wait, after the halving"
        );
    }

    #[test]
    fn the_wait_grows_as_four_to_the_version() {
        use super::optimization_budget_after;
        assert_eq!(optimization_budget_after(240_000, 0), 240_000);
        assert_eq!(optimization_budget_after(240_000, 2), 3_840_000);
        assert_eq!(
            optimization_budget_after(0, 4),
            0,
            "the optimize stress mode's"
        );
        assert_eq!(
            optimization_budget_after(i64::MAX / 2, 3),
            i64::MAX,
            "saturating"
        );
    }

    /// A row for one key, as a run-out makes it.
    fn row_of_a_key() -> (JitSlot, super::CustomizationKey) {
        use crate::mixin::DispatchId;
        let key = super::CustomizationKey {
            receiver: DispatchId::from_counter(DispatchId::FIRST_MINTED),
            entered: DispatchId::from_counter(DispatchId::FIRST_MINTED + 1),
        };
        let slot = JitSlot::default();
        slot.copies.borrow_mut().push(super::CustomizedCopy {
            key,
            body: super::NO_BODY,
            version: 0,
            traps: 0,
            refused: false,
            level: Some(0),
            entered_word: 0,
        });
        (slot, key)
    }

    #[test]
    fn a_decay_halves_a_copy_s_traps() {
        let (slot, key) = row_of_a_key();
        for traps in 1..=4 {
            assert_eq!(slot.count_trap(Some(key)), traps);
            assert_eq!(slot.count_trap(None), traps);
        }
        slot.raise_version(Some(key));
        slot.raise_version(None);
        for _ in 0..4 {
            slot.count_trap(Some(key));
            slot.count_trap(None);
        }
        slot.decay_exits();
        let row = slot.copy_row(key).expect("the row");
        assert_eq!(row.traps, 2);
        assert_eq!(slot.optimized_traps.get(), 2);
        assert_eq!(row.version, 1, "a version never decays");
        assert_eq!(slot.optimized_version.get(), 1);
    }

    /// A key's versions climb to one past `LAST_VERSION`, each counting
    /// its traps from none, and a failure there refuses the key for good,
    /// as it refuses the uncustomized copy.
    #[test]
    fn a_failure_past_the_last_version_refuses_for_good() {
        use super::LAST_VERSION;
        let (slot, key) = row_of_a_key();
        for version in 1..=LAST_VERSION + 1 {
            slot.count_trap(Some(key));
            slot.count_trap(None);
            assert_eq!(slot.raise_version(Some(key)), Some(version));
            assert_eq!(slot.raise_version(None), Some(version));
            assert_eq!(slot.copy_row(key).expect("the row").traps, 0);
            assert_eq!(slot.optimized_traps.get(), 0);
        }
        assert_eq!(slot.optimized_version(Some(key)), LAST_VERSION + 1);
        assert!(!slot.copy_row(key).expect("the row").refused);
        assert!(!slot.optimized_refused.get());
        assert_eq!(slot.raise_version(Some(key)), None);
        assert_eq!(slot.raise_version(None), None);
        let row = slot.copy_row(key).expect("the row");
        assert!(row.refused);
        assert_eq!(row.version, LAST_VERSION + 1, "kept where it failed");
        assert!(slot.optimized_refused.get());
    }

    #[test]
    fn a_copy_speculates_up_to_its_last_version() {
        use super::optimizing::Speculation;
        use super::LAST_VERSION;
        assert_eq!(Speculation::at_version(0), Speculation::Traps);
        assert_eq!(Speculation::at_version(LAST_VERSION), Speculation::Traps);
        assert_eq!(Speculation::at_version(LAST_VERSION + 1), Speculation::None);
    }

    #[test]
    fn a_push_counts_an_abandoned_body_inline_and_arms_only_the_revival() {
        let slot = JitSlot::default();
        for _ in 0..5 {
            slot.abandon(super::REVIVAL_ENTRIES);
        }
        assert_eq!(slot.entries_before_revival(), 65_536, "the longest wait");
        for entry in 1..65_536 {
            assert!(!slot.push_is_armed(), "entry {entry} is counted inline");
        }
        assert_eq!(slot.entries_before_revival(), 1);
        assert!(slot.push_is_armed(), "the entry that revives it");
        assert!(slot.push_is_armed(), "until `entered` revives it");
        assert!(slot.is_abandoned());
    }

    #[test]
    fn an_abandoned_body_is_unarmed_and_its_compile_counts_exits_afresh() {
        let slot = JitSlot::default();
        let offsets = [ns_memory::CodeOffset(64), ns_memory::CodeOffset(80)];
        slot.compiled(7, offsets, 3);
        for _ in 0..super::EXIT_BUDGET - 1 {
            assert!(!slot.note_exit());
        }
        assert!(slot.note_exit(), "the budget is spent");
        slot.abandon(super::REVIVAL_ENTRIES);
        assert!(!slot.push_is_armed(), "a push counts its entry inline");
        assert!(!slot.is_compiled());
        assert_eq!(slot.entries_before_revival(), 4095);
        assert_eq!(format!("{slot:?}"), "JitSlot::Abandoned(64)");
        slot.compiled(8, offsets, 4);
        assert!(
            !slot.note_exit(),
            "a compile into another zone counts afresh"
        );
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
        assert!(slot.push_is_armed(), "a push enters every compiled body");
        assert!(slot.is_compiled());
        assert_eq!(format!("{:?}", slot.clone()), "JitSlot::Counting(0)");
        slot.refuse();
        assert!(!slot.push_is_armed());
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
        use ns_memory::CodeOffset;

        use crate::jit::{compile_twin, install, machine_entry, note_entry, JitState};

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

        /// An interpreter with the JIT on and the optimize stress mode off,
        /// whatever the environment says, for the tests that pin the
        /// budget's own arithmetic and the second entry's compile, both of
        /// which the mode overrides by design: what the boot did under it
        /// -- its requests and their optimizations -- forgotten, so that
        /// the counts are the test's own.
        fn pinning_the_budget() -> Interpreter {
            let mut interpreter = Interpreter::bootstrap_with(crate::Lowering::Optimized);
            interpreter.set_jit_enabled(true);
            interpreter.set_optimize_stress(false);
            interpreter.enter(|mutator| {
                mutator.jit.optimization_requests.clear();
                mutator.jit.statistics = super::super::JitStatistics::default();
            });
            interpreter
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

        /// The size of a zone with `room` bytes past its stubs: a test that
        /// fills a zone fixes the room, since the stubs' size is the
        /// architecture's and its ABI's.
        fn zone_bytes_with_room(room: usize) -> usize {
            let state = JitState::for_test(1 << 20, true);
            let zone = state.zone.as_ref().expect("a zone");
            zone.next_offset as usize + room
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
            assert!(code.jit.push_is_armed(), "a push enters it");
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
                assert!(!code.jit.push_is_armed(), "a push leaves it alone");
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
            assert!(code.jit.push_is_armed(), "a push enters it");
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
            assert!(
                !code.jit.push_is_armed(),
                "a later push pays only the compare"
            );
            assert_eq!(state.statistics().refused_bodies, 1);
            let _ = state.set_enabled(true);
            note_entry(&mut state, &code, FeedbackBase::NOT_ASSIGNED, None);
            assert_eq!(machine_entry(&state, &code), None, "not revived");
            assert_eq!(state.statistics().refused_bodies, 1);
            assert_eq!(state.statistics().compiled_bodies, 0);
        }

        /// An abandoned body counts its pushes inline and reaches `entered`
        /// only at the one that would revive it, so switching the JIT off
        /// refuses it at once: no push pays the count while the JIT is off,
        /// and turning it on again revives nothing entered meanwhile.
        #[test]
        fn a_disabled_jit_refuses_an_abandoned_body_for_good() {
            let mut state = JitState::for_test(1 << 16, true);
            let code = body();
            enter_twice(&mut state, &code);
            assert!(machine_entry(&state, &code).is_some());
            code.jit.abandon(state.revival_base);
            state.abandoned.push(Rc::downgrade(&code));
            let refused = state.statistics().refused_bodies;
            let _ = state.set_enabled(false);
            assert_eq!(format!("{:?}", code.jit), "JitSlot::Refused");
            assert_eq!(state.statistics().refused_bodies, refused + 1);
            let _ = state.set_enabled(true);
            for _ in 0..super::super::REVIVAL_ENTRIES {
                assert!(!code.jit.push_is_armed(), "a refused push pays the compare");
            }
            assert!(!state.decay_exits(), "nothing to revive");
            assert_eq!(format!("{:?}", code.jit), "JitSlot::Refused");
        }

        /// A replaced zone keeps an abandoned body abandoned, to be revived
        /// into the new zone; a replacement that leaves the JIT off, its
        /// zone not reserved, refuses it as switching the JIT off does.
        #[test]
        fn a_replaced_zone_refuses_an_abandoned_body_only_when_the_jit_is_left_off() {
            let mut state = JitState::for_test(1 << 16, true);
            let code = body();
            enter_twice(&mut state, &code);
            code.jit.abandon(state.revival_base);
            state.abandoned.push(Rc::downgrade(&code));
            state.replace_zone(1 << 16);
            assert!(state.is_enabled());
            assert!(
                code.jit.is_abandoned(),
                "kept, to be revived into the new zone"
            );
            let refused = state.statistics().refused_bodies;
            state.replace_zone(16);
            assert!(!state.is_enabled(), "the stubs do not fit");
            assert_eq!(format!("{:?}", code.jit), "JitSlot::Refused");
            assert_eq!(state.statistics().refused_bodies, refused + 1);
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
        fn a_zone_laid_out_with_paddings_starts_its_stubs_and_bodies_there() {
            let writer = ns_memory::CodeZone::reserve(1 << 16).expect("a zone");
            let mut zone = crate::jit::zone_with_paddings(writer, 1 << 16, 208, 48).expect("fits");
            assert_eq!(zone.entry_stub.0, 208, "the first stub");
            let plain = ns_memory::CodeZone::reserve(1 << 16).expect("a zone");
            let plain = crate::jit::zone_with_paddings(plain, 1 << 16, 0, 0).expect("fits");
            assert_eq!(zone.bodies_start, plain.bodies_start + 208 + 48);
            assert_eq!(zone.next_offset, zone.bodies_start);
            let first = install(&mut zone, &[0x90; 8]).expect("room");
            assert_eq!(first.0, zone.bodies_start, "the first body, padded");
        }

        #[test]
        fn the_protocol_s_three_builds_have_distinct_phases() {
            // (stub padding, body padding) of builds a, b and c
            // (`measuring-performance`).
            let builds = [(0u32, 0u32), (208, 32), (96, 176)];
            for (index, (stubs, bodies)) in builds.iter().enumerate() {
                assert!(stubs % 16 == 0 && bodies % 16 == 0);
                for (other_stubs, other_bodies) in &builds[index + 1..] {
                    assert_ne!(stubs % 64, other_stubs % 64, "the stubs' phases");
                    assert_ne!(
                        (stubs + bodies) % 64,
                        (other_stubs + other_bodies) % 64,
                        "the bodies' phases"
                    );
                }
            }
        }

        #[test]
        fn a_padding_is_a_multiple_of_sixteen_up_to_4096() {
            use crate::jit::padding_from;
            assert_eq!(padding_from(None), 0);
            assert_eq!(padding_from(Some("0")), 0);
            assert_eq!(padding_from(Some("16")), 16);
            assert_eq!(padding_from(Some("4096")), 4096);
            for text in ["17", "4112", "x", ""] {
                let outcome = std::panic::catch_unwind(|| padding_from(Some(text)));
                assert!(outcome.is_err(), "{text:?} is refused");
            }
        }

        /// A lookup by an address inside the code, which the step,
        /// initializer and own-slot leaves make, searches the installed
        /// bodies once per address, and its remembered answer is never
        /// trusted past the body's being freed.
        #[test]
        fn a_lookup_by_a_code_address_searches_once_and_never_answers_a_freed_body() {
            let mut state = JitState::for_test(1 << 16, false);
            let first = body_with_a_loop();
            let second = body_with_a_loop();
            enter_twice(&mut state, &first);
            enter_twice(&mut state, &second);
            let zone = state.zone.as_ref().expect("a zone");
            let address = |code: &Rc<CompiledCode>| {
                let start = machine_entry(&state, code).expect("compiled");
                zone.writer.zone().executable_address(start) + 8
            };
            let (inside_first, inside_second) = (address(&first), address(&second));
            let searches = |state: &JitState| state.body_lookup.searches.get();
            // The offset `installed_index_at` keys the entry by: the byte
            // before the address, as a return address names its call.
            let offset_of = |state: &JitState, address: usize| {
                let zone = state.zone.as_ref().expect("a zone");
                let base = zone.writer.zone().executable_address(CodeOffset(0));
                u32::try_from(address - base - 1).expect("inside the zone")
            };
            assert_eq!(state.installed_index_at(inside_first), Some(0));
            assert_eq!(searches(&state), 1);
            for _ in 0..3 {
                assert_eq!(state.installed_index_at(inside_first), Some(0));
            }
            assert_eq!(searches(&state), 1, "answered without a search");
            assert_eq!(state.installed_index_at(inside_second), Some(1));
            assert_eq!(state.installed_index_at(inside_second), Some(1));
            assert_eq!(searches(&state), 2);
            // An entry naming a live body that no longer holds the address
            // -- as after a compaction renumbered `bodies` -- or no body at
            // all is not taken: the search answers the body holding it.
            for stale in [2, 99] {
                state
                    .body_lookup
                    .entry(offset_of(&state, inside_first))
                    .set(stale);
                assert_eq!(
                    state.installed_index_at(inside_first),
                    Some(0),
                    "the body holding the address, not the remembered {}",
                    stale - 1
                );
            }
            assert_eq!(searches(&state), 4);
            state.bodies[0].free = true;
            assert_eq!(state.installed_index_at(inside_first), None, "freed");
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

        /// A body's row is armed with the budget when its baseline code is
        /// installed, before the code runs, and its data word names the
        /// row; a spend that ran the armed row out requests the body's
        /// optimization once and parks the row, while a spend from the
        /// unpointed budget, or from a row cleared since, points the word
        /// and arms the row and requests nothing.
        #[test]
        fn a_budget_that_runs_out_requests_its_body_once() {
            let mut interpreter = pinning_the_budget();
            interpreter.enter(|mutator| {
                let code = body_with_a_loop();
                enter_twice(&mut mutator.jit, &code);
                crate::jit::after_compile_trigger(mutator);
                assert_eq!(
                    mutator.heap.armed_optimization_budget(&code),
                    Some(crate::jit::OPTIMIZATION_BUDGET)
                );
                let index = code.jit.map.get();
                let word = |mutator: &crate::interpreter::Mutator<'_>| {
                    let body = &mutator.jit.bodies[index as usize];
                    let offset = body.start + body.budget_word.expect("a budget word");
                    let zone = mutator.jit.zone.as_ref().expect("a zone");
                    let bytes = zone
                        .writer
                        .bytes(offset..offset + 8)
                        .expect("committed")
                        .to_vec();
                    u64::from_le_bytes(bytes.try_into().expect("a word"))
                };
                let row = mutator.heap.optimization_budget_of(&code).address;
                assert_eq!(word(mutator), row, "pointed at install");
                let inside = {
                    let body = &mutator.jit.bodies[index as usize];
                    let zone = mutator.jit.zone.as_ref().expect("a zone");
                    zone.writer
                        .zone()
                        .executable_address(CodeOffset(body.start + 1))
                };
                let unpointed = mutator.jit.unpointed_budget_address();
                crate::jit::budget_spent(mutator, unpointed, 0, inside);
                assert!(
                    mutator.jit.optimization_requests.is_empty(),
                    "a spend from the unpointed budget"
                );
                assert_eq!(word(mutator), row, "pointed again");
                crate::jit::budget_spent(mutator, row, 0, inside);
                crate::jit::budget_spent(mutator, row, 0, inside);
                assert_eq!(mutator.jit.optimization_requests.len(), 1);
                assert_eq!(mutator.jit.statistics.optimization_requests, 1, "once");
                assert_eq!(
                    mutator.heap.armed_optimization_budget(&code),
                    Some(crate::heap::OPTIMIZATION_BUDGET_PARKED)
                );
                mutator.heap.forget_optimization_budget(&code);
                crate::jit::budget_spent(mutator, row, 0, inside);
                assert_eq!(
                    mutator.heap.armed_optimization_budget(&code),
                    Some(crate::jit::OPTIMIZATION_BUDGET),
                    "a cleared row is armed afresh, requesting nothing"
                );
                assert_eq!(mutator.jit.statistics.optimization_requests, 1);
            });
        }

        /// A requested body's optimized code is installed beside its
        /// baseline code at the safepoint: a push and a compiled call
        /// enter the optimized code, a frame that goes on mid-body the
        /// baseline code, and freeing the baseline code frees the
        /// optimized code with it, so that no lookup finds it.
        #[test]
        fn optimized_code_is_entered_at_the_start_and_baseline_code_mid_body() {
            let mut interpreter = pinning_the_budget();
            interpreter.enter(|mutator| {
                let code = body_with_a_loop();
                enter_twice(&mut mutator.jit, &code);
                crate::jit::after_compile_trigger(mutator);
                let base = FeedbackBase::NOT_ASSIGNED;
                let lean = code.jit.map.get();
                let baseline_start = mutator.jit.bodies[lean as usize].start;
                let baseline_push = mutator.jit.interpreter_entry(&code, base, 0);
                assert!(mutator.jit.optimized(&code).is_none());
                assert_eq!(
                    mutator.jit.machine_entry_of(&code, base, None),
                    Some(CodeOffset(baseline_start))
                );
                mutator
                    .jit
                    .optimization_requests
                    .push(crate::jit::OptimizationRequest::uncustomized(&code));
                crate::jit::optimize_requested(mutator);
                assert!(mutator.jit.optimization_requests.is_empty());
                assert_eq!(mutator.jit.statistics.optimized_bodies, 1);
                let optimized = code.jit.optimized.get();
                let body = &mutator.jit.bodies[optimized as usize];
                assert_eq!(body.tier, crate::jit::Tier::Optimized);
                assert_eq!(body.baseline, Some(lean));
                let optimized_start = body.start;
                assert_eq!(
                    mutator.jit.machine_entry_of(&code, base, None),
                    Some(CodeOffset(optimized_start)),
                    "a compiled call enters the optimized prologue"
                );
                assert_eq!(
                    mutator.jit.optimized_push_entry(&code, base, None),
                    Some(CodeOffset(optimized_start + body.push_entry)),
                    "a push enters the optimized code past its prologue"
                );
                assert_eq!(
                    mutator.jit.interpreter_entry(&code, base, 0),
                    baseline_push,
                    "a frame that goes on mid-body, at the loop's head, enters baseline code"
                );
                // A second request finds the optimized code installed.
                mutator
                    .jit
                    .optimization_requests
                    .push(crate::jit::OptimizationRequest::uncustomized(&code));
                crate::jit::optimize_requested(mutator);
                assert_eq!(mutator.jit.statistics.optimized_bodies, 1);
                // Freed whenever its baseline body is.
                mutator.jit.free_body(lean);
                assert!(mutator.jit.bodies[optimized as usize].free);
                code.jit.forget();
                assert!(mutator.jit.optimized(&code).is_none());
                assert_eq!(code.jit.optimized.get(), crate::jit::NO_BODY);
            });
        }

        /// No room for the optimized code is no refusal of the body: as a
        /// baseline compile that finds the zone full, the optimizing one
        /// asks for a compaction and records what it needed, and the body's
        /// parked budget is armed afresh, so that it is requested again.
        #[test]
        fn a_full_zone_at_the_optimizing_install_compacts_and_rearms_the_budget() {
            let mut interpreter = pinning_the_budget();
            interpreter.enter(|mutator| {
                // Bodies before the hot one, so that a zone without room
                // left would still hold its optimized code once compacted.
                let fillers: Vec<_> = (0..4).map(|_| body_with_a_loop()).collect();
                let hot = body_with_a_loop();
                for code in fillers.iter().chain([&hot]) {
                    enter_twice(&mut mutator.jit, code);
                }
                crate::jit::after_compile_trigger(mutator);
                let zone = mutator.jit.zone.as_mut().expect("a zone");
                zone.limit = zone.next_offset;
                let budget = mutator.heap.optimization_budget_of(&hot);
                mutator.heap.park_optimization_budget(budget.id);
                mutator
                    .jit
                    .optimization_requests
                    .push(crate::jit::OptimizationRequest::uncustomized(&hot));
                crate::jit::optimize_requested(mutator);
                assert_eq!(mutator.jit.statistics.optimized_bodies, 0);
                assert!(mutator.jit.optimized(&hot).is_none());
                assert!(mutator.jit.compaction_requested());
                assert!(mutator.jit.zone_exhausted);
                assert!(mutator.jit.wanted_bytes > 0);
                assert_eq!(
                    mutator.heap.armed_optimization_budget(&hot),
                    Some(crate::jit::OPTIMIZATION_BUDGET),
                    "armed afresh"
                );
            });
        }

        /// While the zone stays exhausted, a body whose optimized code
        /// found no room is not snapshotted, compiled and emitted again
        /// until the room left holds what it needed: the request is held,
        /// counting towards the next compaction, and its budget armed
        /// afresh, as the baseline and twin compiles hold theirs.
        #[test]
        fn an_exhausted_zone_holds_an_optimization_known_not_to_fit() {
            let mut interpreter = pinning_the_budget();
            interpreter.enter(|mutator| {
                let hot = body_with_a_loop();
                enter_twice(&mut mutator.jit, &hot);
                crate::jit::after_compile_trigger(mutator);
                let zone = mutator.jit.zone.as_mut().expect("a zone");
                zone.limit = zone.next_offset;
                let lean = hot.jit.map.get();
                mutator
                    .jit
                    .optimization_requests
                    .push(crate::jit::OptimizationRequest::uncustomized(&hot));
                crate::jit::optimize_requested(mutator);
                let needed = mutator.jit.bodies[lean as usize].optimized_needed;
                assert!(needed > 0, "what it needed is recorded");
                assert!(mutator.jit.zone_exhausted);
                let (held, spent) = (
                    mutator.jit.held_compiles,
                    mutator.jit.statistics.optimize_nanoseconds,
                );
                let budget = mutator.heap.optimization_budget_of(&hot);
                mutator.heap.park_optimization_budget(budget.id);
                mutator
                    .jit
                    .optimization_requests
                    .push(crate::jit::OptimizationRequest::uncustomized(&hot));
                crate::jit::optimize_requested(mutator);
                assert_eq!(mutator.jit.held_compiles, held + 1, "held");
                assert_eq!(
                    mutator.jit.statistics.optimize_nanoseconds, spent,
                    "nothing compiled"
                );
                assert_eq!(
                    mutator.heap.armed_optimization_budget(&hot),
                    Some(crate::jit::OPTIMIZATION_BUDGET),
                    "armed afresh"
                );
                // With the room it needed, it is compiled.
                let zone = mutator.jit.zone.as_mut().expect("a zone");
                zone.limit = zone.next_offset + needed;
                mutator
                    .jit
                    .optimization_requests
                    .push(crate::jit::OptimizationRequest::uncustomized(&hot));
                crate::jit::optimize_requested(mutator);
                assert!(mutator.jit.optimized(&hot).is_some());
                assert_eq!(mutator.jit.bodies[lean as usize].optimized_needed, 0);
            });
        }

        /// A body whose optimized code the zone could not hold even empty
        /// is refused for good: once the refusal is recorded, a budget
        /// armed afresh -- as a recompile of its baseline code arms one --
        /// runs out without requesting the body, and a request still
        /// pending compiles nothing.
        #[test]
        fn an_optimization_refused_for_good_is_never_requested_again() {
            let mut interpreter = pinning_the_budget();
            interpreter.enter(|mutator| {
                let code = body_with_a_loop();
                enter_twice(&mut mutator.jit, &code);
                crate::jit::after_compile_trigger(mutator);
                let zone = mutator.jit.zone.as_mut().expect("a zone");
                zone.limit = zone.bodies_start + 1;
                mutator
                    .jit
                    .optimization_requests
                    .push(crate::jit::OptimizationRequest::uncustomized(&code));
                crate::jit::optimize_requested(mutator);
                assert!(mutator.jit.optimized(&code).is_none());
                assert!(code.jit.optimized_refused.get(), "the refusal is recorded");
                let spent = mutator.jit.statistics.optimize_nanoseconds;
                let index = code.jit.map.get();
                let inside = {
                    let body = &mutator.jit.bodies[index as usize];
                    let zone = mutator.jit.zone.as_ref().expect("a zone");
                    zone.writer
                        .zone()
                        .executable_address(CodeOffset(body.start + 1))
                };
                let row = mutator.heap.optimization_budget_of(&code).address;
                mutator.heap.forget_optimization_budget(&code);
                crate::jit::budget_spent(mutator, row, 0, inside);
                assert_eq!(
                    mutator.heap.armed_optimization_budget(&code),
                    Some(crate::jit::OPTIMIZATION_BUDGET),
                    "armed afresh"
                );
                let requests = mutator.jit.statistics.optimization_requests;
                crate::jit::budget_spent(mutator, row, 0, inside);
                assert!(mutator.jit.optimization_requests.is_empty());
                assert_eq!(
                    mutator.jit.statistics.optimization_requests, requests,
                    "not requested"
                );
                assert_eq!(
                    mutator.heap.armed_optimization_budget(&code),
                    Some(crate::heap::OPTIMIZATION_BUDGET_PARKED),
                    "parked"
                );
                mutator
                    .jit
                    .optimization_requests
                    .push(crate::jit::OptimizationRequest::uncustomized(&code));
                crate::jit::optimize_requested(mutator);
                assert_eq!(
                    mutator.jit.statistics.optimize_nanoseconds, spent,
                    "nothing compiled"
                );
            });
        }

        /// Optimized code that spends its exit budget is abandoned alone:
        /// its exits are its own, never the baseline code's, its baseline
        /// code stays installed and entered at its start again, and each
        /// abandonment raises its version, the budget armed at four times
        /// the last; abandoned past its last version, it is refused for
        /// good, and a request compiles nothing.
        #[test]
        fn optimized_code_that_keeps_exiting_is_abandoned_alone_and_backs_off() {
            let mut interpreter = pinning_the_budget();
            interpreter.enter(|mutator| {
                let code = body_with_a_loop();
                enter_twice(&mut mutator.jit, &code);
                crate::jit::after_compile_trigger(mutator);
                let base = FeedbackBase::NOT_ASSIGNED;
                let lean = code.jit.map.get();
                let baseline_start = mutator.jit.bodies[lean as usize].start;
                let budget = crate::jit::OPTIMIZATION_BUDGET;
                let last = crate::jit::LAST_VERSION;
                for abandonments in 1..=last + 2 {
                    mutator
                        .jit
                        .optimization_requests
                        .push(crate::jit::OptimizationRequest::uncustomized(&code));
                    crate::jit::optimize_requested(mutator);
                    let optimized = code.jit.optimized.get();
                    assert_ne!(optimized, crate::jit::NO_BODY, "optimized again");
                    for _ in 0..crate::jit::EXIT_BUDGET - 1 {
                        crate::jit::count_exit(mutator, &code, true, None);
                    }
                    assert!(mutator.jit.optimized(&code).is_some(), "not yet");
                    crate::jit::count_exit(mutator, &code, true, None);
                    assert!(mutator.jit.bodies[optimized as usize].free);
                    assert!(mutator.jit.optimized(&code).is_none());
                    assert_eq!(code.jit.optimized.get(), crate::jit::NO_BODY);
                    assert!(!mutator.jit.bodies[lean as usize].free, "baseline stays");
                    assert_eq!(code.jit.exits.get(), 0, "the baseline's exits untouched");
                    assert_eq!(
                        mutator.jit.machine_entry_of(&code, base, None),
                        Some(CodeOffset(baseline_start)),
                        "a compiled call enters the baseline code again"
                    );
                    if abandonments <= last + 1 {
                        assert_eq!(
                            mutator.heap.armed_optimization_budget(&code),
                            Some(budget * 4_i64.pow(u32::from(abandonments))),
                            "abandonment {abandonments}"
                        );
                        assert!(!code.jit.optimized_refused.get());
                    }
                    assert_eq!(
                        mutator.jit.statistics.abandoned_optimized_bodies,
                        u64::from(abandonments)
                    );
                }
                assert!(code.jit.optimized_refused.get(), "refused for good");
                assert_eq!(mutator.jit.statistics.refused_for_good, 1);
                let spent = mutator.jit.statistics.optimize_nanoseconds;
                mutator
                    .jit
                    .optimization_requests
                    .push(crate::jit::OptimizationRequest::uncustomized(&code));
                crate::jit::optimize_requested(mutator);
                assert_eq!(
                    mutator.jit.statistics.optimize_nanoseconds, spent,
                    "nothing compiled"
                );
                assert_eq!(mutator.jit.statistics.abandoned_bodies, 0);
            });
        }

        /// An optimization put off for want of room keeps the back-off
        /// its body's abandoned optimized code earned: the budget is armed
        /// afresh at four times the plain one after one abandonment, both
        /// when the install finds no room and when the exhausted zone
        /// holds it.
        #[test]
        fn a_deferred_optimization_keeps_its_back_off() {
            let mut interpreter = pinning_the_budget();
            interpreter.enter(|mutator| {
                let code = body_with_a_loop();
                enter_twice(&mut mutator.jit, &code);
                crate::jit::after_compile_trigger(mutator);
                mutator
                    .jit
                    .optimization_requests
                    .push(crate::jit::OptimizationRequest::uncustomized(&code));
                crate::jit::optimize_requested(mutator);
                for _ in 0..crate::jit::EXIT_BUDGET {
                    crate::jit::count_exit(mutator, &code, true, None);
                }
                assert_eq!(code.jit.optimized_version.get(), 1);
                let backed_off = Some(crate::jit::OPTIMIZATION_BUDGET * 4);
                let zone = mutator.jit.zone.as_mut().expect("a zone");
                zone.limit = zone.next_offset;
                for held in [false, true] {
                    let budget = mutator.heap.optimization_budget_of(&code);
                    mutator.heap.park_optimization_budget(budget.id);
                    mutator
                        .jit
                        .optimization_requests
                        .push(crate::jit::OptimizationRequest::uncustomized(&code));
                    crate::jit::optimize_requested(mutator);
                    assert!(mutator.jit.optimized(&code).is_none());
                    assert_eq!(
                        mutator.heap.armed_optimization_budget(&code),
                        backed_off,
                        "held: {held}"
                    );
                }
            });
        }

        /// Baseline code that spends its exit budget is abandoned, and
        /// its optimized code is freed with it: the pair goes, and no
        /// lookup finds either.
        #[test]
        fn an_abandoned_baseline_body_frees_its_optimized_code_too() {
            let mut interpreter = Interpreter::bootstrap_with(crate::Lowering::Optimized);
            interpreter.set_jit_enabled(true);
            interpreter.enter(|mutator| {
                let code = body_with_a_loop();
                enter_twice(&mut mutator.jit, &code);
                crate::jit::after_compile_trigger(mutator);
                let lean = code.jit.map.get();
                mutator
                    .jit
                    .optimization_requests
                    .push(crate::jit::OptimizationRequest::uncustomized(&code));
                crate::jit::optimize_requested(mutator);
                let optimized = code.jit.optimized.get();
                assert_ne!(optimized, crate::jit::NO_BODY);
                for _ in 0..crate::jit::EXIT_BUDGET {
                    crate::jit::count_exit(mutator, &code, false, None);
                }
                assert!(mutator.jit.bodies[lean as usize].free);
                assert!(mutator.jit.bodies[optimized as usize].free);
                assert!(code.jit.is_abandoned());
                assert!(mutator.jit.installed(&code).is_none());
                assert!(mutator.jit.optimized(&code).is_none());
                assert_eq!(mutator.jit.statistics.abandoned_bodies, 1);
                assert_eq!(mutator.jit.statistics.abandoned_optimized_bodies, 0);
            });
        }

        /// `total`, a method of `A`, run on an `A` by `runA` and on a `B`,
        /// a subclass of `A`, by `runB`.
        const ONE_METHOD_ON_TWO_APPLICATIONS: &str = indoc::indoc! {"
            class Subject = Object new () (
              class A = Object new (
                |
                  step = 2.
                |
              ) (
                public total = (
                  |
                    sum ::= 0.
                    index ::= 0.
                  |
                  [index < 100] whileTrue: [
                    sum:: sum + step.
                    index:: index + 1.
                  ].
                  ^sum.
                )
              )

              class B = A new () ()

              public aClass = (
                ^A.
              )

              public bClass = (
                ^B.
              )

              public runA = (
                ^A new total.
              )

              public runB = (
                ^B new total.
              )
            ) : ()
        "};

        /// Whether the environment turns on a stress mode that may free
        /// the copies a run made before the test reads them.
        fn a_stress_mode_frees_copies() -> bool {
            [
                "ZONE",
                "COLLECT",
                "SCAVENGE",
                "MOVES",
                "SUSPEND",
                "EXIT",
                "CACHES",
                "INVALIDATE",
            ]
            .iter()
            .any(|mode| {
                std::env::var_os(format!("NS_NEXT_STRESS_{mode}"))
                    .is_some_and(|value| !value.is_empty() && value != "0")
            })
        }

        /// The answer of `subject`'s unary `selector`.
        fn sent(
            interpreter: &mut Interpreter,
            subject: &crate::Root,
            selector: &str,
        ) -> crate::Root {
            match interpreter.send(subject, crate::Symbol::intern(selector), &[]) {
                Ok(root) => root,
                Err(_) => panic!("{selector}: an error"),
            }
        }

        /// A cell guarded by the receiver's own id names the copy whose key
        /// its guard and its target's level make -- the receiver's
        /// application and the application the level reaches above it --
        /// and the baseline code while that key has no copy; a `super`
        /// cell, whose guard tells nothing of the receiver, names the
        /// body's dispatching entry once it has a copy.
        #[test]
        fn a_receiver_guarded_cell_names_its_copy() {
            use crate::interpreter::inline_cache::{CacheEntry, Target};
            use crate::interpreter::resolve::Chain;
            use crate::jit::{cell_entry, CustomizationKey};

            let source = ONE_METHOD_ON_TWO_APPLICATIONS;
            let module = ns_syntax::parse(source).expect("parse");
            let resolved = crate::resolve(
                module,
                Rc::from(source),
                crate::types::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
            )
            .expect("resolve");
            let mut interpreter = Interpreter::bootstrap_with(crate::Lowering::Optimized);
            interpreter.set_jit_enabled(true);
            interpreter.set_jit_stress(true);
            interpreter.set_optimize_stress(true);
            let class = interpreter.load_module(&resolved);
            let Ok(subject) = interpreter.instantiate(&class, &[]) else {
                panic!("instantiate: an error");
            };
            sent(&mut interpreter, &subject, "runA");
            let base_class = sent(&mut interpreter, &subject, "aClass");
            let subclass = sent(&mut interpreter, &subject, "bClass");
            let check = |interpreter: &mut Interpreter, subclass_has_a_copy: bool| {
                interpreter.enter(|mutator| {
                    let base_object = mutator.fetch(&base_class).as_object().expect("a class");
                    let subclass_object = mutator.fetch(&subclass).as_object().expect("a class");
                    let (base_id, subclass_id) = (
                        mutator.heap.class_dispatch_ids(base_object).instances,
                        mutator.heap.class_dispatch_ids(subclass_object).instances,
                    );
                    let application = mutator
                        .heap
                        .class_application(base_object)
                        .expect("applied");
                    let method = application
                        .mixin
                        .compiled_method(crate::Symbol::intern("total"))
                        .expect("total")
                        .clone();
                    let code = Rc::clone(&method.code);
                    let feedback_base = mutator.feedback_base_for(&code);
                    let entry = |guard, level, chain| CacheEntry {
                        guard,
                        target: Target::Method {
                            method: Rc::clone(&method),
                            feedback_base,
                            level,
                            chain,
                        },
                    };
                    let jit = &mutator.jit;
                    let zone = jit.zone.as_ref().expect("a zone");
                    let address = |start: u32| {
                        zone.writer.zone().executable_address(CodeOffset(start)) as u64
                    };
                    let baseline = address(jit.installed(&code).expect("compiled").start);
                    let copy = |receiver| {
                        let key = CustomizationKey {
                            receiver,
                            entered: base_id,
                        };
                        jit.copy_of(&code, key).map(|body| address(body.start))
                    };
                    let base_copy = copy(base_id).expect("A's copy");
                    assert_eq!(
                        cell_entry(jit, &entry(base_id, 0, Chain::Public)),
                        base_copy
                    );
                    // A `super` cell's guard says nothing of the receiver:
                    // it names the body's dispatching entry, which the
                    // receiver's copy is reached through.
                    assert_eq!(
                        Some(cell_entry(jit, &entry(base_id, 0, Chain::Super))),
                        jit.dispatching_entry_address(&code),
                        "a super cell names the dispatching entry"
                    );
                    match copy(subclass_id) {
                        Some(subclass_copy) => {
                            assert!(subclass_has_a_copy);
                            assert_ne!(subclass_copy, base_copy);
                            assert_eq!(
                                cell_entry(jit, &entry(subclass_id, 1, Chain::Public)),
                                subclass_copy
                            );
                        }
                        None => {
                            assert!(!subclass_has_a_copy);
                            assert_eq!(
                                cell_entry(jit, &entry(subclass_id, 1, Chain::Public)),
                                baseline,
                                "B's baseline code before B's copy exists"
                            );
                        }
                    }
                });
            };
            if a_stress_mode_frees_copies() {
                return;
            }
            check(&mut interpreter, false);
            sent(&mut interpreter, &subject, "runB");
            check(&mut interpreter, true);
        }

        /// `M`'s private lazy slot `step`, read by `total` in a loop, `M`
        /// applied twice in `Twice`'s chain, its slot holding 10 at the
        /// upper level and 1 at the lower one.
        const ONE_MIXIN_APPLIED_TWICE: &str = indoc::indoc! {"
            class Subject = Object new () (
              class Base = Object new () ()

              class M step: value = Object new (
                |
                  private seed = value.
                |
              ) (
                lazy private step = seed.

                public total = (
                  |
                    sum ::= 0.
                    index ::= 0.
                  |
                  [index < 100] whileTrue: [
                    sum:: sum + step.
                    index:: index + 1.
                  ].
                  ^sum.
                )
              )

              class Middle = Object new () (
                public lowerTotal = (
                  ^super total.
                )
              )

              class Twice = Base new <: M step: 1 <: Middle <: M step: 10.

              public mixinClass = (
                ^M.
              )

              public twiceClass = (
                ^Twice.
              )

              public run = (
                |
                  twice = Twice new.
                |
                ^twice total + twice lowerTotal.
              )
            ) : ()
        "};

        /// A mixin applied twice has a copy of its lazy slot's getter per
        /// level, each reading its own level's field, and a cell guarded by
        /// the receiver's id names, at each level, that level's copy: the
        /// level reaches the entered application the copy's key names.
        #[test]
        fn a_receiver_guarded_cell_at_each_level_names_that_level_s_copy() {
            use crate::interpreter::inline_cache::{CacheEntry, Target};
            use crate::interpreter::resolve::Chain;
            use crate::jit::{cell_entry, CustomizationKey};

            let source = ONE_MIXIN_APPLIED_TWICE;
            let module = ns_syntax::parse(source).expect("parse");
            let resolved = crate::resolve(
                module,
                Rc::from(source),
                crate::types::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
            )
            .expect("resolve");
            let mut interpreter = Interpreter::bootstrap_with(crate::Lowering::Optimized);
            interpreter.set_jit_enabled(true);
            interpreter.set_jit_stress(true);
            interpreter.set_optimize_stress(true);
            let class = interpreter.load_module(&resolved);
            let Ok(subject) = interpreter.instantiate(&class, &[]) else {
                panic!("instantiate: an error");
            };
            for _ in 0..3 {
                let answer = sent(&mut interpreter, &subject, "run");
                let answer = interpreter.enter(|mutator| mutator.fetch(&answer).as_small_integer());
                assert_eq!(answer, Some(1100), "each level reads its own slot");
            }
            if a_stress_mode_frees_copies() {
                return;
            }
            let mixin_class = sent(&mut interpreter, &subject, "mixinClass");
            let twice_class = sent(&mut interpreter, &subject, "twiceClass");
            interpreter.enter(|mutator| {
                let mixin_object = mutator.fetch(&mixin_class).as_object().expect("a class");
                let twice_object = mutator.fetch(&twice_class).as_object().expect("a class");
                let mixin = Rc::clone(
                    &mutator
                        .heap
                        .class_application(mixin_object)
                        .expect("applied")
                        .mixin,
                );
                let method = mixin
                    .compiled_method(crate::Symbol::intern("step"))
                    .expect("the getter")
                    .clone();
                let code = Rc::clone(&method.code);
                let feedback_base = mutator.feedback_base_for(&code);
                let receiver = mutator.heap.class_dispatch_ids(twice_object).instances;
                // The levels of `Twice`'s chain that apply `M`, by their
                // steps from `Twice`, and the id each is entered at.
                let mut levels = Vec::new();
                let mut current = Some(twice_object);
                let mut steps: u16 = 0;
                while let Some(level) = current {
                    if let Some(application) = mutator.heap.class_application(level) {
                        if Rc::ptr_eq(&application.mixin, &mixin) {
                            levels.push((steps, application.dispatch_ids.instances));
                        }
                    }
                    current = mutator.heap.class_superclass(level);
                    steps += 1;
                }
                assert_eq!(levels.len(), 2, "M is applied twice");
                let jit = &mutator.jit;
                let zone = jit.zone.as_ref().expect("a zone");
                let address =
                    |start: u32| zone.writer.zone().executable_address(CodeOffset(start)) as u64;
                let mut copies = Vec::new();
                for (level, entered) in levels {
                    let key = CustomizationKey { receiver, entered };
                    let copy = jit
                        .copy_of(&code, key)
                        .map(|body| address(body.start))
                        .expect("the level's copy");
                    let entry = CacheEntry {
                        guard: receiver,
                        target: Target::Method {
                            method: Rc::clone(&method),
                            feedback_base,
                            level,
                            chain: Chain::Public,
                        },
                    };
                    assert_eq!(cell_entry(jit, &entry), copy, "level {level}");
                    copies.push(copy);
                }
                assert_ne!(copies[0], copies[1], "a copy per level");
            });
        }

        /// A body that makes a closure, so that its lean code transfers its
        /// frame into its twin, but returns 7 from its first named local
        /// before it does.
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

        /// The return of a frame running its body's twin divorces the
        /// frame object in machine code -- inline, or through the divorce
        /// leaf for one that retains its registers -- as the pop divorces
        /// it, and leaves the frame unreified, so that the run loop's pop
        /// after it divorces nothing again: the frame the run loop
        /// entered machine code at, reified before, as the interpreter
        /// reifies a frame an exit left, which enters the twin.
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

        /// A frame the interpreter reified -- as a sender walk, or the
        /// interpreter's run of a `MakeClosure` an exit handed over,
        /// reifies a frame an exit left -- goes on in machine code only in
        /// its body's twin, compiled for it even for a body that makes no
        /// closure: the twin's sends store their program counters and its
        /// return divorces the frame, so that the frame object records the
        /// program counter the interpreter would have, as the lean code,
        /// which stores none and divorces nothing, left it to the run
        /// loop's pop to record the one the frame was resumed at. A frame
        /// unreified goes on in the lean code, which compiles no twin.
        #[test]
        fn a_frame_the_interpreter_reified_resumes_in_the_twin() {
            let mut interpreter = Interpreter::bootstrap_with(crate::Lowering::Optimized);
            interpreter.set_jit_enabled(true);
            interpreter.enter(|mutator| {
                let code = body();
                note_entry(&mut mutator.jit, &code, FeedbackBase::NOT_ASSIGNED, None);
                note_entry(&mut mutator.jit, &code, FeedbackBase::NOT_ASSIGNED, None);
                let resume = |mutator: &mut crate::interpreter::Mutator<'_>, reified: bool| {
                    let activation = push_factory(mutator, &code);
                    let frame = reified.then(|| mutator.heap.reify(activation));
                    // The program counter of the last send the frame
                    // stopped at, as the exit's walk wrote it.
                    mutator.heap.activation_set_program_counter(activation, 1);
                    let register_base = mutator.heap.activation_register_base(activation);
                    let crate::jit::Resumption::Returned { value } =
                        crate::jit::resumed(mutator, activation, &code, register_base, 0)
                    else {
                        panic!("the body runs to its return in machine code");
                    };
                    assert_eq!(
                        value,
                        Immediate::small_integer(3).expect("a small integer").bits()
                    );
                    let divorced = frame.map(|frame| {
                        (
                            mutator.heap.frame_activation(frame).is_none(),
                            mutator.heap.frame_register_shape(frame).0,
                        )
                    });
                    mutator.heap.pop_activation(activation);
                    divorced
                };
                assert_eq!(resume(mutator, false), None);
                assert_eq!(
                    mutator.jit.statistics.compiled_twins, 0,
                    "an unreified frame goes on in the lean code"
                );
                assert_eq!(
                    resume(mutator, true),
                    Some((true, 1)),
                    "the twin's return divorced the frame at the program counter it stored"
                );
                assert_eq!(mutator.jit.statistics.compiled_twins, 1);
                assert_eq!(resume(mutator, true), Some((true, 1)));
                assert_eq!(
                    mutator.jit.statistics.compiled_twins, 1,
                    "the twin is compiled once"
                );
            });
        }

        /// Pushes a factory's activation of `code`, as the run loop pushes
        /// one: a top-level activation is never entered in machine code.
        fn push_factory(
            mutator: &mut crate::interpreter::Mutator<'_>,
            code: &Rc<CompiledCode>,
        ) -> crate::heap::ActivationReference {
            let class = mutator.builtin_class(crate::types::BuiltinClass::Object);
            mutator
                .heap
                .push_activation(
                    crate::heap::ActivationHeader {
                        receiver: crate::Value::object(class),
                        current_class: Some(class),
                        kind: crate::types::ActivationKind::Factory(class),
                        method: None,
                        feedback_base: FeedbackBase::NOT_ASSIGNED,
                        code: Rc::clone(code),
                        then: crate::types::Continuation::Deliver,
                        counted: true,
                    },
                    crate::interpreter::Arguments::NONE,
                    code.registers_above_header(),
                )
                .unwrap_or_else(|_| panic!("the stack holds the frame"))
        }

        /// A body's data words lie in the zone past its last instruction,
        /// on a cache line no instruction shares, before the next body's
        /// code, and zeroed at install, so that their first run fills
        /// them -- the lean body's transfer word, until the twin is
        /// compiled, and the twin's closure site -- but for the lean body's
        /// budget word, which names the unpointed budget until a state
        /// with a heap points it. The compile of the twin writes the
        /// transfer word with the twin's address of the `MakeClosure`.
        #[test]
        fn data_words_live_with_their_body() {
            let mut state = JitState::for_test(1 << 16, true);
            let making = body_making_a_closure();
            let next = body();
            enter_twice(&mut state, &making);
            enter_twice(&mut state, &next);
            assert_eq!(state.statistics().compiled_bodies, 2);
            let alignment = crate::jit::macro_assembler::BODY_WORDS_ALIGNMENT;
            let words = |state: &JitState, index: usize| {
                let body = &state.bodies[index];
                let zone = state.zone.as_ref().expect("a zone");
                zone.writer
                    .bytes(body.words..body.words_end)
                    .expect("committed")
                    .to_vec()
            };
            assert!(
                words(&state, 0)[8..].iter().all(|byte| *byte == 0),
                "the transfer word zeroed at install"
            );
            let twin = compile_twin(&mut state, 0).expect("the twin fits");
            assert_eq!(twin, 2, "installed after the bodies before it");
            assert_eq!(compile_twin(&mut state, 0), Ok(twin), "compiled once");
            assert_eq!(state.statistics().compiled_twins, 1);
            for (index, word_bytes) in [(0, 16), (1, 8), (2, 24)] {
                let body = &state.bodies[index];
                assert!(body.words < body.end, "the body has data words");
                assert_eq!(body.words_end - body.words, word_bytes);
                assert_eq!(body.words % alignment, 0, "the words start a line");
                assert_eq!(body.end % alignment, 0, "the words end a line");
                let last_instruction = body
                    .map
                    .starts
                    .iter()
                    .map(|offset| body.start + offset)
                    .max()
                    .expect("an instruction");
                assert!(last_instruction < body.words, "past the body's code");
            }
            let (first, second, twin) = (&state.bodies[0], &state.bodies[1], &state.bodies[2]);
            assert!(first.end <= second.start, "before the next body's");
            assert!(second.end <= twin.start, "the twin after both");
            assert_eq!(
                second.budget_word,
                Some(second.words - second.start),
                "a body without transfers has its budget word alone"
            );
            assert!(!first.reifies && twin.reifies);
            assert_eq!((first.twin, twin.lean), (Some(2), Some(0)), "paired");
            assert!(
                words(&state, 2).iter().all(|byte| *byte == 0),
                "the twin's site is zeroed at install"
            );
            let zone = state.zone.as_ref().expect("a zone");
            let expected = zone.writer.zone().executable_address(CodeOffset(
                twin.start + twin.map.machine_offset(2).expect("mapped"),
            )) as u64;
            let mut lean_words = state.unpointed_budget_address().to_le_bytes().to_vec();
            lean_words.extend_from_slice(&expected.to_le_bytes());
            assert_eq!(
                words(&state, 0),
                lean_words,
                "the budget word naming the unpointed budget, and the transfer the twin's"
            );
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

        /// A twin the zone could not hold even empty is refused for good:
        /// the refusal is recorded on the body's slot, and the twin is
        /// never assembled again, so that the next exit or resumption that
        /// needed it refuses the body (`refuse_for_its_twin`).
        #[test]
        fn a_twin_too_large_for_the_zone_is_never_assembled_again() {
            let mut probe = JitState::for_test(1 << 16, true);
            let making = body_making_a_closure();
            enter_twice(&mut probe, &making);
            compile_twin(&mut probe, 0).expect("a twin fits the probe");
            let (lean, twin) = (&probe.bodies[0], &probe.bodies[1]);
            assert!(
                twin.size() > lean.size(),
                "the twin reifies inline and holds a site of three words"
            );
            let bytes = lean.end as usize;
            let mut state = JitState::for_test(bytes, true);
            let code = body_making_a_closure();
            enter_twice(&mut state, &code);
            assert!(machine_entry(&state, &code).is_some(), "the lean body fits");
            assert_eq!(
                compile_twin(&mut state, 0),
                Err(crate::jit::Refusal::TooLarge)
            );
            assert!(code.jit.twin_refused.get(), "the refusal is recorded");
            assert_eq!(
                compile_twin(&mut state, 0),
                Err(crate::jit::Refusal::TooLarge),
                "and answered without assembling the twin again"
            );
            assert_eq!(state.statistics().compiled_twins, 0);
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
            statistics.stack_growths = after.stack_growths - before.stack_growths;
            let disturbances = interpreter.enter(|mutator| mutator.heap.seat_disturbances());
            (answer, statistics, disturbances)
        }

        /// A closure stub's copies of a block row's arguments, which lie
        /// past the sender's window before the block's prologue checks its
        /// frame's room, are made only below the ordinary end, and a
        /// prologue's frame only below it or after the growth leaf raised
        /// it: at the end the call declines or the leaf grows the stack
        /// first, the sentinel each seat writes right above the end stays
        /// whole until then, and the program answers the interpreter's
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
            assert!(
                statistics.exits + statistics.stack_growths > 0,
                "the calls at the edge are refused, or grow the stack first"
            );
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
            let mut state = JitState::for_test(zone_bytes_with_room(1_344), true);
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
            let mut state = JitState::for_test(zone_bytes_with_room(1_344), true);
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

/// Machine code for an architecture other than the host's: every stub and
/// every body the shipped core library and the language goldens compile
/// to, assembled for AAPCS64 on whatever host the suite runs on, so that
/// an operation the AArch64 encoder cannot lay out is found without an
/// AArch64 machine.
#[cfg(all(test, ns_jit))]
mod cross_assembly_tests {
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use super::macro_assembler::Abi;
    use super::{assemble, baseline, stubs, targets, CompileSettings, FeedbackBase};
    use crate::bytecode::{CompiledCode, Instruction};
    use crate::mixin::{MethodEntry, Mixin, MixinMethod};
    use crate::Lowering;

    /// The bytes of an assembled stub are whole AArch64 instructions.
    fn assert_instructions(name: &str, bytes: &[u8]) {
        assert!(!bytes.is_empty(), "the {name} stub has code");
        assert_eq!(bytes.len() % 4, 0, "the {name} stub is whole instructions");
    }

    #[test]
    fn every_stub_assembles_for_aapcs64() {
        let abi = Abi::Aapcs64;
        let stubs = [
            ("entry", stubs::assemble_entry_stub(abi)),
            ("call", stubs::assemble_call_stub(abi)),
            ("closure call", stubs::assemble_closure_call_stub(abi)),
            ("exit", stubs::assemble_exit_stub(abi)),
            ("cut", stubs::assemble_cut_stub(abi)),
            ("miss", stubs::assemble_miss_stub(abi)),
            ("refusing", stubs::assemble_refusing_stub(abi)),
            ("polymorphic", targets::assemble_polymorphic_stub(abi)),
            ("megamorphic", targets::assemble_megamorphic_stub(abi)),
            (
                "dispatching",
                targets::assemble_dispatching_stub(abi, false),
            ),
            (
                "counting dispatching",
                targets::assemble_dispatching_stub(abi, true),
            ),
        ];
        for (name, assembled) in &stubs {
            assert_instructions(name, &assembled.bytes);
        }
        // The accessor stubs are one piece of code and the offsets of its
        // entries, each of which a cell jumps to, so each must start an
        // instruction inside it.
        let (accessors, entries) = targets::assemble_accessor_stubs(abi);
        assert_instructions("accessor", &accessors.bytes);
        // Destructured, so that an entry added later is checked too.
        let targets::AccessorEntries {
            getters,
            setters,
            parameters,
            mirror_methods,
            row_stubs,
            nil,
            true_constant,
            false_constant,
            receiver,
            act,
            guarded_getters,
            guarded_setters,
            nested_classes,
            closure_values,
            values_of_parameters,
            culls,
        } = entries;
        let offsets = [
            getters,
            setters,
            parameters,
            mirror_methods,
            row_stubs,
            guarded_getters,
            guarded_setters,
            nested_classes,
            closure_values,
            values_of_parameters,
            culls,
        ]
        .into_iter()
        .flatten()
        .chain([nil, true_constant, false_constant, receiver, act]);
        let length = accessors.bytes.len();
        for offset in offsets {
            let offset = usize::try_from(offset).expect("an offset");
            assert!(
                offset < length,
                "an accessor entry at {offset} lies inside the code"
            );
            assert_eq!(
                offset % 4,
                0,
                "the accessor entry at {offset} starts an instruction"
            );
        }
    }

    /// A body to compile: its code, and the method it is the body of when
    /// it is one's.
    type Body = (Rc<CompiledCode>, Option<Rc<MixinMethod>>);

    fn collect(code: &Rc<CompiledCode>, method: Option<&Rc<MixinMethod>>, into: &mut Vec<Body>) {
        for instruction in code.instructions() {
            if let Instruction::MakeClosure { code: block, .. } = instruction {
                collect(&block, None, into);
            }
        }
        into.push((Rc::clone(code), method.cloned()));
    }

    fn collect_mixin(mixin: &Rc<Mixin>, into: &mut Vec<Body>) {
        for entry in mixin.methods.values() {
            match entry {
                MethodEntry::Compiled(method) => collect(&method.code, Some(method), into),
                MethodEntry::Factory(factory) => collect(&factory.code, None, into),
                MethodEntry::SlotAccessor(_) | MethodEntry::Primitive(_) => {}
            }
        }
        for code in [
            &mixin.initializer_code,
            &mixin.own_initializer_code,
            &mixin.standalone_initializer_code,
            &mixin.superclass_clause_code,
        ]
        .into_iter()
        .flatten()
        {
            collect(code, None, into);
        }
        if let Some(class_side) = &mixin.class_side {
            collect_mixin(class_side, into);
        }
        for nested in mixin.declared_nested().iter() {
            collect_mixin(nested, into);
        }
    }

    fn sources_under(directory: &Path, into: &mut Vec<PathBuf>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(directory)
            .expect("a directory")
            .map(|entry| entry.expect("an entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                sources_under(&path, into);
            } else if path.extension().is_some_and(|extension| extension == "ns") {
                into.push(path);
            }
        }
    }

    /// Every body of the shipped core library and of the programs under
    /// `goldens/language/` that resolve, compiled under `lowering`.
    fn golden_bodies(lowering: Lowering) -> Vec<Body> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        sources_under(&root.join("goldens/language"), &mut files);
        let mut modules = vec![crate::interpreter::kernel::resolved_kernel()];
        for shipped in crate::interpreter::core_library::SHIPPED_MODULES {
            modules.push(crate::interpreter::core_library::resolved_shipped_module(
                shipped.name,
            ));
        }
        for file in &files {
            let source = std::fs::read_to_string(file).expect("readable");
            // A golden that does not parse or resolve records that error
            // and compiles nothing.
            let resolved = ns_syntax::parse(&source).ok().and_then(|module| {
                crate::resolve(
                    module,
                    Rc::from(source.as_str()),
                    crate::SourceOrigin::File(file.clone()),
                )
                .ok()
            });
            if let Some(resolved) = resolved {
                modules.push(Rc::new(resolved));
            }
        }
        let mut interpreter = crate::Interpreter::bootstrap_with(lowering);
        let mut bodies = Vec::new();
        for module in &modules {
            collect_mixin(&interpreter.compile_module(module), &mut bodies);
        }
        bodies
    }

    #[test]
    fn every_golden_body_compiles_for_aapcs64() {
        let mut compiled = 0usize;
        let mut failures = Vec::new();
        for lowering in [Lowering::Optimized, Lowering::Reference] {
            for (code, method) in golden_bodies(lowering) {
                for counting in [false, true] {
                    let settings = CompileSettings {
                        counting,
                        futures_exit: false,
                    };
                    for copy in [baseline::BodyCopy::Lean, baseline::BodyCopy::Twin] {
                        match assemble(
                            Abi::Aapcs64,
                            settings,
                            &code,
                            FeedbackBase::from_raw(0),
                            method.as_ref(),
                            copy,
                        ) {
                            Ok(body) => {
                                assert_eq!(body.code.bytes.len() % 4, 0);
                                compiled += 1;
                            }
                            Err(refusal) => failures.push(format!(
                                "{lowering:?} {copy:?} counting {counting}: {refusal:?}\n{}",
                                crate::bytecode::disassemble(&code, "body")
                            )),
                        }
                    }
                }
            }
        }
        for failure in failures.iter().take(3) {
            eprintln!("{failure}");
        }
        assert!(failures.is_empty(), "{} bodies failed", failures.len());
        eprintln!("{compiled} bodies compiled");
        assert!(compiled > 10_000, "only {compiled} bodies");
    }
}
