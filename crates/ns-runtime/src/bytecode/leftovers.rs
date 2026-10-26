//! What a callee may leave in its sender's scratch registers: one forward
//! pass over a body's instructions, over bitsets (`docs/design/compiler.md`,
//! "What a callee leaves").
//!
//! A send's callee is laid on its argument slots, so its header and window
//! go over every register of the sender above the slots, and after the
//! send returns those registers hold whatever the callee left there. Every
//! send but `SendMirror`, whose leaf's activation lands above its sender's
//! window, therefore *taints* the registers at and above its slot end. A
//! send writes none of its slots for certain -- the run loop reads one
//! argument, or consecutive ones, where they stand, and a primitive or an
//! accessor answers without a push -- so a slot keeps whatever status it
//! had. A write of a register cleans it. Every scratch register is tainted
//! at entry, since a push fills none of them. Registers below
//! `local_count` -- the arguments, the header's numbers and the named
//! locals -- are never tainted: no callee is laid below the scratch, and
//! a push fills them.
//!
//! The pass keeps one current bitset of the tainted registers. A forward
//! jump ORs it into its target's pending bitset; reaching that target ORs
//! the pending bitset in, or takes it over where control does not fall
//! into the target. A loop head, when reached, is tainted at and above the
//! least slot end of any send in the loop, which is what every later
//! iteration can bring there; the loop's own code cannot taint below it,
//! so the back edge needs no second pass.
//!
//! An image is untrusted input, and the decoder runs this on every body it
//! reads, so the pass is bounded before it allocates: it refuses a body of
//! more than `INSTRUCTION_LIMIT` instructions, more than `PENDING_LIMIT`
//! targets pending at once, more than `LOOP_LIMIT` loops open at once,
//! loops that cross, and a jump into a loop past its head. None of them
//! refuses a body either lowering emits (the proof is
//! `docs/design/compiler.md`, "What a callee leaves").
//!
//! The decoder's refusal (`image::decode`'s `verify_body`) and a debug
//! build's check of every compiled body call `check`; the collector's
//! pre-pass (`Heap::trace_stack`) asks `tainted_at` through each body's
//! `TaintedSets`, which keeps what it answered.

use std::cell::{Cell, Ref, RefCell};
use std::ops::Range;

use super::{
    falls_through, jump_target, register_written, registers_read, CompiledCode, Instruction,
    INSTRUCTION_LIMIT,
};

/// The parser's nesting limit (`ns_syntax`'s `MAX_DEPTH`): how many guarded
/// productions -- a primary, a block literal among them -- may be open at
/// once. Pinned by `the_parser_accepts_no_deeper_nesting`.
pub(crate) const PARSER_NESTING_LIMIT: usize = 128;

/// How many forward jump targets a compiled body keeps pending at once, at
/// most.
///
/// Every forward jump the optimized lowering emits belongs to an inlined
/// form (`emit_guarded_branch`, `lower_while`, `lower_mirror_send`,
/// `lower_integer_binary`); the reference lowering emits none. Over the
/// code inside one of its block literals a form keeps at most two of its
/// own jumps open, and over its own instructions at most three: a guarded
/// branch's guard, its false branch and its true arm's exit, just as the
/// exit opens. Every form's own instructions lie inside at most
/// `PARSER_NESTING_LIMIT - 1` block literals, since its operands or its
/// blocks take one guarded production more. So at most `2 × 127 + 3` jumps
/// are open, the most of a guarded branch at the deepest point a form can
/// stand at, and no more targets than jumps are pending
/// (`docs/design/compiler.md`, "What a callee leaves").
pub(crate) const PENDING_LIMIT: usize = 2 * (PARSER_NESTING_LIMIT - 1) + 3;

/// How many loops a compiled body has open at once -- nested, each inside
/// the one around it -- at most.
///
/// Every loop either lowering emits is an inlined `whileTrue:` or one of
/// its relatives (`lower_while`, one back edge each); the reference
/// lowering emits none. A loop's code is its condition's and its body's
/// block literals, so a loop nested in another lies inside one of the
/// outer one's block literals, and the innermost of `depth` nested loops
/// lies inside `depth - 1` of them. Its own instructions lie inside at most
/// `PARSER_NESTING_LIMIT - 1` block literals (`PENDING_LIMIT`), so at most
/// `PARSER_NESTING_LIMIT` loops are open (`docs/design/compiler.md`, "What
/// a callee leaves").
pub(crate) const LOOP_LIMIT: usize = PARSER_NESTING_LIMIT;

/// A loop's floor that stands for none: a loop without a send. No slot end
/// the pass keeps reaches it, since a slot end is below `register_count`.
const NO_FLOOR: u16 = u16::MAX;

/// Why a body is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeftoverRefusal {
    /// `instruction` reads `register`, which may hold a callee's leftover
    /// or a word the push left there.
    Read { instruction: u32, register: u16 },
    /// At `instruction` the body passes what no compiled body reaches:
    /// more instructions than `INSTRUCTION_LIMIT`, more than
    /// `PENDING_LIMIT` targets pending, more than `LOOP_LIMIT` loops open,
    /// loops that cross, or a jump into a loop past its head.
    PastTheBound { instruction: u32 },
}

/// Checks `instructions`, the body of a code whose first scratch register
/// is `local_count` and whose register file is `register_count` long, and
/// keeps nothing: what the image decoder runs.
pub(crate) fn check(
    instructions: &[Instruction],
    local_count: u16,
    register_count: u16,
) -> Result<(), LeftoverRefusal> {
    analyze(instructions, local_count, register_count, &[]).map(|_| ())
}

/// The registers that may hold a callee's leftover, or a word the push
/// left there, just before each instruction of `program_counters`, which
/// is ascending: sorted disjoint ranges, none for an instruction no path
/// reaches, from one pass. What the collector's pre-pass asks of a body,
/// through its `TaintedSets`, the first time it meets a frame running the
/// body at a program counter.
///
/// A frame is asked at the program counter its header stores. Every
/// instruction that hands control to a frame above its own stores its
/// own index first -- a send (`vm::send`, `vm::send_mirror`), an
/// instruction that forces a future (`vm::force_and_reexecute`), and an
/// instruction that raises an error (`vm::raise`), below whose `signal`
/// the frame stays while the handler runs -- so a frame below the top is
/// asked before the instruction it stopped at, and the registers at and
/// above a send's slot end are its callee's window meanwhile. It resumes
/// after a send once the send's destination is written
/// (`vm::resume_caller`), runs the instruction again once the future is
/// resolved (`vm::reexecute`), and never runs again after a raise, since
/// an `Error` is not resumable; a restarted target starts
/// at 0 (`interpreter::unwind`'s `arrive`). The frame on top stores the
/// instruction it runs next when it leaves for a safepoint (`vm::run_stretch`'s
/// write-back). Either way the answer is what the frame does not read
/// before it writes it.
///
/// It is asked only of a body `check` accepted, at program counters within
/// it: the pre-pass nils what it answers, so an answer past the truth
/// would nil a live register, and `TaintedSets` checks a body before it
/// asks. Of any other body, and at a program counter past the body, which
/// a debug build refuses, it answers every scratch register, what the
/// decoder may assume.
pub(crate) fn tainted_at(
    instructions: &[Instruction],
    local_count: u16,
    register_count: u16,
    program_counters: &[u32],
) -> Vec<Vec<Range<u16>>> {
    debug_assert!(
        program_counters
            .iter()
            .all(|program_counter| (*program_counter as usize) < instructions.len()),
        "internal: a program counter past a body of {} instructions",
        instructions.len()
    );
    let scratch = local_count.min(register_count)..register_count;
    let every: Vec<Range<u16>> = if scratch.is_empty() {
        Vec::new()
    } else {
        vec![scratch]
    };
    match analyze(instructions, local_count, register_count, program_counters) {
        Ok(mut analysis) => {
            for (answer, program_counter) in analysis.answers.iter_mut().zip(program_counters) {
                if *program_counter as usize >= instructions.len() {
                    answer.clone_from(&every);
                }
            }
            analysis.answers
        }
        Err(_) => vec![every; program_counters.len()],
    }
}

/// The tainted sets the collector's pre-pass has asked of one body, each
/// under the program counter it was asked at, filled from `tainted_at` the
/// first time a frame stands there and kept for every collection after:
/// a frame's cost is then a lookup and the writes of its nils. Only the
/// sets are kept, never a pass's bitsets.
///
/// What it can reach, per body: one entry per program counter a frame was
/// met at, so at most one per instruction, each 24 bytes and 4 per range
/// of its set; a set's ranges are disjoint and apart, so it holds at most
/// half the body's scratch registers as ranges, plus one -- in practice
/// one or two. A body of `n` instructions and `s` scratch registers thus
/// keeps at most `n` entries of `24 + 4 (s / 2 + 1)` bytes, however many
/// collections ask. It lives as long as its body, which a full collection
/// gives back once nothing names it (`heap/tables.rs`).
#[derive(Default)]
pub(crate) struct TaintedSets {
    /// Whether `check` accepted the body, asked at the first fill: the
    /// shipped code is decoded from the embedded artifact unverified, and
    /// a set of a body `check` refuses would nil live registers.
    checked: Cell<bool>,
    /// Each program counter asked, ascending, and the set before it.
    sets: RefCell<Vec<TaintedSet>>,
}

/// The registers tainted before one program counter.
struct TaintedSet {
    program_counter: u32,
    registers: Box<[Range<u16>]>,
}

impl TaintedSets {
    /// The registers of `code` -- whose sets these are -- tainted before
    /// `program_counter`, as `tainted_at` answers them.
    pub(crate) fn before(
        &self,
        code: &CompiledCode,
        program_counter: u32,
    ) -> Ref<'_, [Range<u16>]> {
        let found = self
            .sets
            .borrow()
            .binary_search_by_key(&program_counter, |set| set.program_counter);
        let position = match found {
            Ok(position) => position,
            Err(position) => {
                // The fill inserts exactly this set, where the search says.
                self.fill_all(code, &[program_counter]);
                position
            }
        };
        Ref::map(self.sets.borrow(), |sets| &*sets[position].registers)
    }

    /// The set before `program_counter` if it is kept already, so that
    /// asking it runs no pass; `None` if no fill has kept it.
    pub(crate) fn kept(&self, program_counter: u32) -> Option<Ref<'_, [Range<u16>]>> {
        Ref::filter_map(self.sets.borrow(), |sets| {
            sets.binary_search_by_key(&program_counter, |set| set.program_counter)
                .ok()
                .map(|position| &*sets[position].registers)
        })
        .ok()
    }

    /// Keeps the sets of `code` before each of `program_counters`, which
    /// is ascending, from one pass over the body, checking the body first
    /// if no fill has: what the collector's pre-pass asks for every
    /// program counter its walk met a frame of the body at and found no
    /// set for, so that a collection decodes and analyses a body once
    /// however many frames run it. A set kept already stays as it is.
    #[cold]
    #[inline(never)]
    pub(crate) fn fill_all(&self, code: &CompiledCode, program_counters: &[u32]) {
        if program_counters.is_empty() {
            return;
        }
        let instructions = code.instructions();
        if !self.checked.get() {
            if let Err(refusal) = check(&instructions, code.local_count, code.register_count) {
                panic!(
                    "internal: the leftover analysis refuses a running body ({refusal:?}), \
                     which every compiled, decoded and shipped body passes"
                );
            }
            self.checked.set(true);
        }
        let answers = tainted_at(
            &instructions,
            code.local_count,
            code.register_count,
            program_counters,
        );
        let mut sets = self.sets.borrow_mut();
        for (program_counter, registers) in program_counters.iter().zip(answers) {
            if let Err(position) =
                sets.binary_search_by_key(program_counter, |set| set.program_counter)
            {
                sets.insert(
                    position,
                    TaintedSet {
                        program_counter: *program_counter,
                        registers: registers.into_boxed_slice(),
                    },
                );
            }
        }
    }
}

/// A copy of a body is asked afresh.
impl Clone for TaintedSets {
    fn clone(&self) -> TaintedSets {
        TaintedSets::default()
    }
}

impl std::fmt::Debug for TaintedSets {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TaintedSets")
    }
}

/// What the pass spent on one body.
#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "read by the tests that bound what the pass spends"
    )
)]
pub(crate) struct Spent {
    /// The words of one bitset: `register_count` rounded up to 64 bits.
    pub(crate) words: usize,
    /// The most bitsets held at once, the current one included.
    pub(crate) peak_bitsets: usize,
    /// The most targets pending at once.
    pub(crate) peak_pending: usize,
    /// The most loops open at once.
    pub(crate) peak_open_loops: usize,
    /// The most bytes of bitsets held at once.
    pub(crate) peak_bytes: u64,
    /// The most word operations on bitsets one instruction took.
    pub(crate) most_word_operations: u64,
}

/// What one pass found: the answers to `tainted_at`'s program counters,
/// and what it spent.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "read by the tests that bound what the pass spends"
    )
)]
pub(crate) struct Analysis {
    pub(crate) answers: Vec<Vec<Range<u16>>>,
    pub(crate) spent: Spent,
}

/// The pass. Answers the tainted registers before each program counter of
/// `program_counters`, which is ascending.
pub(crate) fn analyze(
    instructions: &[Instruction],
    local_count: u16,
    register_count: u16,
    program_counters: &[u32],
) -> Result<Analysis, LeftoverRefusal> {
    if instructions.len() > INSTRUCTION_LIMIT as usize {
        return Err(LeftoverRefusal::PastTheBound {
            instruction: INSTRUCTION_LIMIT,
        });
    }
    debug_assert!(program_counters.windows(2).all(|pair| pair[0] <= pair[1]));
    let local_count = local_count.min(register_count);
    let regions = loops(instructions, local_count, register_count)?;
    let count = instructions.len();
    let mut pass = Pass::new(register_count);
    let mut current = pass.allocate();
    set_range(
        &mut current,
        usize::from(local_count),
        usize::from(register_count),
    );
    let mut reachable = count > 0;
    // The loops the pass is inside, innermost last, at most `LOOP_LIMIT`.
    let mut open_loops: Vec<usize> = Vec::new();
    // In a debug build, the state each open loop took at its head, indexed
    // by its loop, which every back edge's state is within.
    let mut head_states: Vec<Option<Box<[u64]>>> = if cfg!(debug_assertions) {
        std::iter::repeat_with(|| None)
            .take(regions.len())
            .collect()
    } else {
        Vec::new()
    };
    let mut next_region = 0;
    let mut next_answer = 0;
    let mut answers = Vec::with_capacity(program_counters.len());
    for (index, instruction) in instructions.iter().enumerate() {
        let here = u32::try_from(index)
            .expect("internal: analyze refuses a body past INSTRUCTION_LIMIT before it walks one");
        let mut operations = 0;
        while open_loops
            .last()
            .is_some_and(|&innermost| regions[innermost].end < here)
        {
            if let Some(closed) = open_loops.pop() {
                if let Some(head_state) = head_states.get_mut(closed) {
                    *head_state = None;
                }
            }
        }
        if pass
            .pending
            .last()
            .is_some_and(|entry| entry.target == here)
        {
            if let Some(entry) = pass.pending.pop() {
                // Every path into a loop passes its head: a jump to a point
                // inside it comes from inside it.
                if let Some(&innermost) = open_loops.last() {
                    if regions[innermost].head > entry.earliest_source {
                        return Err(LeftoverRefusal::PastTheBound {
                            instruction: entry.earliest_source,
                        });
                    }
                }
                if reachable {
                    operations += union_into(&mut current, &entry.bits);
                    pass.free.push(entry.bits);
                } else {
                    let unreachable = std::mem::replace(&mut current, entry.bits);
                    pass.free.push(unreachable);
                    reachable = true;
                }
            }
        }
        if let Some(region) = regions
            .get(next_region)
            .filter(|region| region.head == here)
        {
            if reachable && region.floor != NO_FLOOR {
                operations += set_range(
                    &mut current,
                    usize::from(region.floor),
                    usize::from(register_count),
                );
            }
            if reachable {
                if let Some(head_state) = head_states.get_mut(next_region) {
                    *head_state = Some(current.clone());
                }
            }
            open_loops.push(next_region);
            pass.spent.peak_open_loops = pass.spent.peak_open_loops.max(open_loops.len());
            next_region += 1;
        }
        while program_counters.get(next_answer) == Some(&here) {
            answers.push(if reachable {
                ranges(&current)
            } else {
                Vec::new()
            });
            next_answer += 1;
        }
        if !reachable {
            continue;
        }
        for register in registers_read(instruction) {
            if contains(&current, register) {
                return Err(LeftoverRefusal::Read {
                    instruction: here,
                    register,
                });
            }
        }
        if let Some(end) = taint_end(instruction, local_count, register_count) {
            operations += set_range(&mut current, usize::from(end), usize::from(register_count));
        }
        if let Some(target) = jump_target(instruction) {
            if target > here && (target as usize) < count {
                operations += pass.open(target, here, &current)?;
            } else if cfg!(debug_assertions) && target <= here {
                check_back_edge(&head_states, &regions, target, &current);
            }
        }
        if !falls_through(instruction) {
            reachable = false;
        } else if let Some(destination) = register_written(instruction) {
            remove(&mut current, destination);
        }
        pass.spent.most_word_operations = pass.spent.most_word_operations.max(operations);
    }
    // Past the body, or never reached.
    answers.resize(program_counters.len(), Vec::new());
    Ok(Analysis {
        answers,
        spent: pass.spent,
    })
}

/// The end of the registers `instruction` taints: its slot end, never below
/// the scratch; `None` for an instruction that is no send, a mirror send,
/// whose leaf lands above the window, and a send whose slots end the
/// register file.
fn taint_end(instruction: &Instruction, local_count: u16, register_count: u16) -> Option<u16> {
    match instruction {
        Instruction::SendMirror { .. } => None,
        other => other
            .argument_slots()
            .map(|(base, sources)| {
                (usize::from(base) + sources.len()).max(usize::from(local_count))
            })
            .filter(|end| *end < usize::from(register_count))
            .map(crate::heap::register_number),
    }
}

/// A loop: its head, its last back edge, and the least slot end of any
/// send between them, `NO_FLOOR` for none.
#[derive(Clone, Copy)]
struct Region {
    head: u32,
    end: u32,
    floor: u16,
}

/// The loops of a body, ordered by head, each with its floor. Refuses
/// loops that cross: a loop's floor stands for every iteration only when
/// the loops inside it lie wholly inside it. Refuses more than
/// `LOOP_LIMIT` open at once, which bounds what a debug build keeps of
/// them.
fn loops(
    instructions: &[Instruction],
    local_count: u16,
    register_count: u16,
) -> Result<Vec<Region>, LeftoverRefusal> {
    let mut regions: Vec<Region> = instructions
        .iter()
        .enumerate()
        .filter_map(|(index, instruction)| {
            jump_target(instruction)
                .filter(|target| *target as usize <= index)
                .map(|head| Region {
                    head,
                    end: u32::try_from(index).expect(
                        "internal: analyze refuses a body past INSTRUCTION_LIMIT before it walks one",
                    ),
                    floor: NO_FLOOR,
                })
        })
        .collect();
    regions.sort_unstable_by_key(|region| (region.head, std::cmp::Reverse(region.end)));
    regions.dedup_by_key(|region| region.head);
    let mut open: Vec<usize> = Vec::new();
    let mut next = 0;
    // An inner loop's sends are its parent's too.
    fn close(open: &mut Vec<usize>, regions: &mut [Region]) {
        if let Some(innermost) = open.pop() {
            if let Some(&parent) = open.last() {
                regions[parent].floor = regions[parent].floor.min(regions[innermost].floor);
            }
        }
    }
    for (index, instruction) in instructions.iter().enumerate() {
        while open
            .last()
            .is_some_and(|&innermost| (regions[innermost].end as usize) < index)
        {
            close(&mut open, &mut regions);
        }
        while next < regions.len() && regions[next].head as usize == index {
            if let Some(&parent) = open.last() {
                if regions[next].end > regions[parent].end {
                    return Err(LeftoverRefusal::PastTheBound {
                        instruction: regions[next].end,
                    });
                }
            }
            if open.len() >= LOOP_LIMIT {
                return Err(LeftoverRefusal::PastTheBound {
                    instruction: regions[next].head,
                });
            }
            open.push(next);
            next += 1;
        }
        if let Some(&innermost) = open.last() {
            if let Some(end) = taint_end(instruction, local_count, register_count) {
                regions[innermost].floor = regions[innermost].floor.min(end);
            }
        }
    }
    while !open.is_empty() {
        close(&mut open, &mut regions);
    }
    Ok(regions)
}

/// A forward jump target not yet reached: the union of the states the
/// jumps to it carried, and the first of them.
struct Pending {
    target: u32,
    earliest_source: u32,
    bits: Box<[u64]>,
}

struct Pass {
    words: usize,
    /// Ordered by target, the nearest last.
    pending: Vec<Pending>,
    /// Bitsets a reached target gave back, for the next target to take:
    /// the pass never holds more than its peak.
    free: Vec<Box<[u64]>>,
    spent: Spent,
}

impl Pass {
    fn new(register_count: u16) -> Pass {
        let words = usize::from(register_count).div_ceil(64);
        Pass {
            words,
            pending: Vec::new(),
            free: Vec::new(),
            spent: Spent {
                words,
                ..Spent::default()
            },
        }
    }

    /// A bitset, a freed one where there is one, its content undefined.
    fn allocate(&mut self) -> Box<[u64]> {
        self.free.pop().unwrap_or_else(|| {
            self.spent.peak_bitsets += 1;
            self.spent.peak_bytes = (self.spent.peak_bitsets * self.words * 8) as u64;
            vec![0; self.words].into_boxed_slice()
        })
    }

    /// Carries `current` to `target` from `source`; answers its word
    /// operations.
    fn open(&mut self, target: u32, source: u32, current: &[u64]) -> Result<u64, LeftoverRefusal> {
        match self
            .pending
            .binary_search_by(|entry| target.cmp(&entry.target))
        {
            Ok(position) => Ok(union_into(&mut self.pending[position].bits, current)),
            Err(position) => {
                if self.pending.len() >= PENDING_LIMIT {
                    return Err(LeftoverRefusal::PastTheBound {
                        instruction: source,
                    });
                }
                let mut bits = self.allocate();
                bits.copy_from_slice(current);
                self.pending.insert(
                    position,
                    Pending {
                        target,
                        earliest_source: source,
                        bits,
                    },
                );
                self.spent.peak_pending = self.spent.peak_pending.max(self.pending.len());
                Ok(self.words as u64)
            }
        }
    }
}

/// A back edge's state is within its head's: what the head's floor stands
/// for. Its loop is found by its head, among the loops ordered by head.
fn check_back_edge(
    head_states: &[Option<Box<[u64]>>],
    regions: &[Region],
    head: u32,
    current: &[u64],
) {
    let Some(head_state) = regions
        .binary_search_by_key(&head, |region| region.head)
        .ok()
        .and_then(|region| head_states.get(region))
        .and_then(Option::as_ref)
    else {
        return;
    };
    for (word, assumed) in current.iter().zip(head_state.iter()) {
        assert!(
            word & !assumed == 0,
            "internal: {:?} is tainted at the back edge to {head} but not at the head",
            ranges(current)
        );
    }
}

/// Sets the registers `start..end` of `bits`; answers the words it touched.
fn set_range(bits: &mut [u64], start: usize, end: usize) -> u64 {
    if start >= end {
        return 0;
    }
    let first = start / 64;
    let last = (end - 1) / 64;
    for (index, word) in bits.iter_mut().enumerate().take(last + 1).skip(first) {
        let low = if index == first { start % 64 } else { 0 };
        let high = if index == last {
            (end - 1) % 64 + 1
        } else {
            64
        };
        let width = high - low;
        let mask = if width == 64 {
            u64::MAX
        } else {
            ((1u64 << width) - 1) << low
        };
        *word |= mask;
    }
    (last - first + 1) as u64
}

/// ORs `source` into `target`; answers the words it touched.
fn union_into(target: &mut [u64], source: &[u64]) -> u64 {
    for (word, other) in target.iter_mut().zip(source.iter()) {
        *word |= other;
    }
    target.len() as u64
}

fn contains(bits: &[u64], register: u16) -> bool {
    let register = usize::from(register);
    bits.get(register / 64)
        .is_some_and(|word| (word >> (register % 64)) & 1 == 1)
}

fn remove(bits: &mut [u64], register: u16) {
    let register = usize::from(register);
    if let Some(word) = bits.get_mut(register / 64) {
        *word &= !(1u64 << (register % 64));
    }
}

/// The first register at or after `from` whose bit is `value`, if any.
fn next_with(bits: &[u64], from: usize, value: bool) -> Option<usize> {
    let mut index = from / 64;
    let mut mask = u64::MAX << (from % 64);
    while let Some(&word) = bits.get(index) {
        let candidates = if value { word } else { !word } & mask;
        if candidates != 0 {
            return Some(index * 64 + candidates.trailing_zeros() as usize);
        }
        index += 1;
        mask = u64::MAX;
    }
    None
}

/// The registers of `bits` as sorted disjoint ranges. No bit at or past
/// `register_count` is ever set, so every range ends within it.
fn ranges(bits: &[u64]) -> Vec<Range<u16>> {
    let total = bits.len() * 64;
    let mut found = Vec::new();
    let mut position = 0;
    while let Some(start) = next_with(bits, position, true) {
        let end = next_with(bits, start, false).unwrap_or(total);
        found.push(crate::heap::register_number(start)..crate::heap::register_number(end));
        position = end;
    }
    found
}

#[cfg(test)]
mod tests;
