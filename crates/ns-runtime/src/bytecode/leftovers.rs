//! What a callee may leave in its sender's scratch registers: a forward
//! analysis over a body's control flow (`docs/design/compiler.md`, "What a
//! callee leaves").
//!
//! A send's callee is laid on its argument slots, so its header and window
//! go over every register of the sender above the slots, and after the
//! send returns those registers hold whatever the callee left there. Every
//! send but `SendMirror`, whose leaf's activation lands above its sender's
//! window, therefore *taints* the registers at and above its slot end; a
//! write of a register cleans it. Registers below `local_count` -- the
//! arguments, the header's numbers and the named locals -- are never
//! tainted: no callee is laid below the scratch.
//!
//! The analysis makes one pass over the instructions in order and keeps one
//! current state. A register is tainted when the last taint covering it is
//! later than its last write: each write records its time, and the taints
//! are a stack of `(end, time)` with rising ends, a later taint popping
//! every one with an end at or above its own. Every change is a record in
//! an undo log, so a forward jump stores no state: it opens a pending entry
//! anchored at the log's length, and at its target the state it carried is
//! the current one with the log above the anchor undone. A join costs what
//! the two paths changed since their common version, never the size of the
//! state. A loop head is tainted, when first reached, at and above the
//! lowest slot end of any send in the loop, which is what every later
//! iteration can bring there; the loop's own code cannot taint below it, so
//! the back edge needs no second pass.
//!
//! An image is untrusted input, and the decoder runs this on every body it
//! reads, so the analysis is bounded: it refuses a body that keeps more
//! forward jumps open at once than any compiled body can
//! (`OPEN_ENTRY_LIMIT`), a loop that is not entered at its head alone or
//! that crosses another, and a body whose work or memory would pass a
//! bound linear in its size. None of them refuses a body either lowering
//! emits (the proof is `docs/design/compiler.md`, "What a callee leaves").
//!
//! Its readers -- the decoder's refusal, a debug build's check of every
//! compiled body, the collector's pre-pass -- are still to come, so only
//! the tests call it yet.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use super::{
    falls_through, jump_target, register_written, registers_read, successors, Instruction,
};

/// The parser's nesting limit (`ns_syntax`'s `MAX_DEPTH`): how many guarded
/// productions -- a primary, a block literal among them -- may be open at
/// once. Every inlined form's code lies in a block literal, so it bounds
/// how deeply inlined forms nest. Pinned by
/// `the_parser_accepts_no_deeper_nesting`.
pub(crate) const PARSER_NESTING_LIMIT: usize = 128;

/// How many forward jumps a compiled body keeps open at once, at most: two
/// for each inlined block enclosing a point, and two for the inlined form
/// whose own instructions stand there (`docs/design/compiler.md`, "What a
/// callee leaves").
pub(crate) const OPEN_ENTRY_LIMIT: usize = 2 * PARSER_NESTING_LIMIT + 2;

/// The work the analysis may spend per unit of a body's size, and beyond
/// it. A unit is an instruction or one register it lists.
pub(crate) const WORK_PER_UNIT: u64 = 16 * OPEN_ENTRY_LIMIT as u64 + 64;
pub(crate) const WORK_ALLOWANCE: u64 = 1 << 16;

/// The bytes the analysis may hold at once, per unit of a body's size, and
/// beyond it.
pub(crate) const MEMORY_PER_UNIT: u64 = 448;
pub(crate) const MEMORY_ALLOWANCE: u64 = 1 << 16;

/// What each thing the pass holds costs, in bytes: a record of the log, a
/// link of a chain, a pending entry with its place in the table of pending
/// entries, an entry of a hash or B-tree table, a taint, and what it keeps
/// per instruction (its effect, its loop floor and head flag, and the
/// loop sweep's two tables while they live).
const RECORD_BYTES: u64 = std::mem::size_of::<Record>() as u64;
const LINK_BYTES: u64 = std::mem::size_of::<(Rc<Vec<Record>>, std::ops::Range<usize>)>() as u64;
const ENTRY_BYTES: u64 = std::mem::size_of::<Entry>() as u64 + 16;
const TABLE_ENTRY_BYTES: u64 = 32;
const TAINT_BYTES: u64 = std::mem::size_of::<Taint>() as u64;
const INSTRUCTION_BYTES: u64 = std::mem::size_of::<Effect>() as u64
    + std::mem::size_of::<Option<u16>>() as u64
    + 1
    + std::mem::size_of::<Option<u32>>() as u64
    + 4;

/// Why a body is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeftoverRefusal {
    /// `instruction` reads `register`, which a callee may have left a value
    /// in.
    Read { instruction: u32, register: u16 },
    /// At `instruction` the body passes what no compiled body reaches: too
    /// many forward jumps open, a loop entered other than at its head or
    /// crossing another, or more work or memory than its size allows.
    PastTheBound { instruction: u32 },
}

/// What the analysis found of one body: enough to answer, for any program
/// counter, which registers may hold a callee's leftovers there.
pub(crate) struct Leftovers {
    effects: Vec<Effect>,
    loop_floors: Vec<Option<u16>>,
    is_loop_head: Vec<bool>,
    local_count: u16,
    register_count: u16,
    /// What the analysis spent and held, for the bound's tests.
    pub(crate) work: u64,
    pub(crate) peak_memory: u64,
    pub(crate) peak_open_entries: usize,
    pub(crate) units: u64,
}

impl Leftovers {
    /// The registers that may hold a callee's leftovers just before the
    /// instruction at `program_counter` runs, in ascending order; none for
    /// an instruction no path reaches. Runs the pass again up to it, so it
    /// costs what `analyze` did, at most.
    pub(crate) fn tainted_before(&self, program_counter: u32) -> impl Iterator<Item = u16> + '_ {
        let mut pass = Pass::new(self.local_count, self.register_count, u64::MAX, u64::MAX, 0);
        let state = pass
            .run(
                &self.effects,
                None,
                &self.loop_floors,
                &self.is_loop_head,
                Some(program_counter),
            )
            .ok()
            .flatten();
        let tainted: Vec<u16> = match state {
            Some(()) => (pass.local_count..pass.register_count)
                .filter(|register| pass.tainted(*register))
                .collect(),
            None => Vec::new(),
        };
        tainted.into_iter()
    }
}

/// Runs the analysis over `instructions`, the body of a code whose first
/// scratch register is `local_count` and whose register file is
/// `register_count` long.
pub(crate) fn analyze(
    instructions: &[Instruction],
    local_count: u16,
    register_count: u16,
) -> Result<Leftovers, LeftoverRefusal> {
    let local_count = local_count.min(register_count);
    let effects: Vec<Effect> = instructions
        .iter()
        .map(|instruction| Effect::of(instruction, local_count, register_count))
        .collect();
    let units: u64 = instructions
        .iter()
        .map(|instruction| {
            1 + registers_read(instruction).count() as u64
                + instruction
                    .argument_slots()
                    .map_or(0, |(_, sources)| sources.len() as u64)
        })
        .sum();
    let (loop_floors, is_loop_head) = loops(instructions, &effects)?;
    let mut pass = Pass::new(
        local_count,
        register_count,
        WORK_PER_UNIT
            .saturating_mul(units)
            .saturating_add(WORK_ALLOWANCE),
        MEMORY_PER_UNIT
            .saturating_mul(units)
            .saturating_add(MEMORY_ALLOWANCE),
        effects.len() as u64 * INSTRUCTION_BYTES,
    );
    pass.run(
        &effects,
        Some(instructions),
        &loop_floors,
        &is_loop_head,
        None,
    )?;
    Ok(Leftovers {
        work: pass.work,
        peak_memory: pass.peak_memory,
        peak_open_entries: pass.peak_open_entries,
        units,
        effects,
        loop_floors,
        is_loop_head,
        local_count,
        register_count,
    })
}

/// What one instruction does to the state, read once from it: the argument
/// slots it writes, the end of the registers it taints, the register it
/// writes after its callee returned, where it may jump, and whether it
/// falls through. Writes below `local_count` are left out, since nothing
/// there is ever tainted.
#[derive(Clone, Copy)]
struct Effect {
    slots: (u16, u16),
    taint: Option<u16>,
    destination: Option<u16>,
    target: Option<u32>,
    falls_through: bool,
}

impl Effect {
    fn of(instruction: &Instruction, local_count: u16, register_count: u16) -> Effect {
        let (slots, taint) = match instruction.argument_slots() {
            Some((base, sources)) => {
                let end =
                    (usize::from(base) + sources.len()).min(usize::from(register_count)) as u16;
                let slots = (base.max(local_count).min(end), end);
                // The callee is laid on the slots and its header where they
                // end; a mirror send's leaf lands above the window instead.
                let taint = match instruction {
                    Instruction::SendMirror { .. } => None,
                    _ => Some(end.max(local_count)).filter(|end| *end < register_count),
                };
                (slots, taint)
            }
            None => ((0, 0), None),
        };
        Effect {
            slots,
            taint,
            destination: register_written(instruction)
                .filter(|register| *register >= local_count && *register < register_count),
            target: jump_target(instruction),
            falls_through: falls_through(instruction),
        }
    }
}

/// The loops of a body: for each loop head, the lowest taint end of any
/// instruction between it and its last back edge, and whether it is one.
/// Refuses loops that cross, and a jump from outside a loop to anywhere in
/// it but its head: the head's taint stands for every iteration only when
/// every path into the loop passes the head.
#[allow(clippy::type_complexity)]
fn loops(
    instructions: &[Instruction],
    effects: &[Effect],
) -> Result<(Vec<Option<u16>>, Vec<bool>), LeftoverRefusal> {
    let count = effects.len();
    let mut last_back_edge: Vec<Option<u32>> = vec![None; count];
    for (index, instruction) in instructions.iter().enumerate() {
        for target in successors(index as u32, instruction) {
            if (target as usize) <= index {
                let slot = &mut last_back_edge[target as usize];
                *slot = Some(slot.map_or(index as u32, |old| old.max(index as u32)));
            }
        }
    }
    let mut floors: Vec<Option<u16>> = vec![None; count];
    let is_head: Vec<bool> = last_back_edge.iter().map(Option::is_some).collect();
    // The innermost loop each instruction lies inside (not at the head of),
    // and each loop's floor, in one sweep over properly nested loops.
    let mut innermost: Vec<u32> = vec![u32::MAX; count];
    let mut open: Vec<(u32, u32, Option<u16>)> = Vec::new();
    for index in 0..count {
        while let Some(&(head, end, floor)) = open.last() {
            if (end as usize) >= index {
                break;
            }
            open.pop();
            floors[head as usize] = floor;
            if let Some(parent) = open.last_mut() {
                parent.2 = lower(parent.2, floor);
            }
        }
        if let Some(&(head, _, _)) = open.last() {
            innermost[index] = head;
        }
        if let Some(end) = last_back_edge[index] {
            if let Some(&(_, parent_end, _)) = open.last() {
                if end > parent_end {
                    return Err(LeftoverRefusal::PastTheBound {
                        instruction: index as u32,
                    });
                }
            }
            open.push((index as u32, end, None));
        }
        if let Some(top) = open.last_mut() {
            top.2 = lower(top.2, effects[index].taint);
        }
    }
    while let Some((head, _, floor)) = open.pop() {
        floors[head as usize] = floor;
        if let Some(parent) = open.last_mut() {
            parent.2 = lower(parent.2, floor);
        }
    }
    for (index, effect) in effects.iter().enumerate() {
        let Some(target) = effect.target else {
            continue;
        };
        let Some(&head) = innermost.get(target as usize) else {
            continue;
        };
        if head == u32::MAX {
            continue;
        }
        let end = last_back_edge[head as usize].unwrap_or(head);
        if (index as u32) < head || (index as u32) > end {
            return Err(LeftoverRefusal::PastTheBound {
                instruction: index as u32,
            });
        }
    }
    Ok((floors, is_head))
}

fn lower(left: Option<u16>, right: Option<u16>) -> Option<u16> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, None) => left,
        (None, right) => right,
    }
}

/// One taint: every register at or above `end` is tainted at `time`.
#[derive(Clone, Copy)]
struct Taint {
    end: u16,
    time: u64,
}

/// One change to the state, with what undoes and what redoes it.
#[derive(Clone, Copy)]
enum Record {
    /// `register`'s write time went from `old` to `new`: a write when
    /// `cleans`, or a join putting back an older time, which taints it.
    Written {
        register: u16,
        old: u64,
        new: u64,
        cleans: bool,
    },
    /// The taint stack's slot `position` took `new`, popping everything
    /// from there up; `old` is the slot's previous content, if it had any,
    /// and `old_length` the stack's previous length.
    Tainted {
        position: u32,
        old: Option<Taint>,
        old_length: u32,
        new: Taint,
    },
}

/// The state of a forward jump not yet reached: the version it carried is
/// the log up to `anchor`, then the records of `chain`, which the current
/// path has since left.
struct Entry {
    anchor: usize,
    chain: Vec<(Rc<Vec<Record>>, std::ops::Range<usize>)>,
}

struct Pass {
    local_count: u16,
    register_count: u16,
    now: u64,
    written: HashMap<u16, u64>,
    stack: Vec<Taint>,
    length: usize,
    log: Vec<Record>,
    pending: BTreeMap<u32, Vec<Entry>>,
    open_entries: usize,
    segments_live: u64,
    chain_links: u64,
    work: u64,
    work_limit: u64,
    peak_memory: u64,
    memory_limit: u64,
    fixed_memory: u64,
    peak_open_entries: usize,
}

impl Pass {
    fn new(
        local_count: u16,
        register_count: u16,
        work_limit: u64,
        memory_limit: u64,
        fixed_memory: u64,
    ) -> Pass {
        Pass {
            local_count,
            register_count,
            now: 0,
            written: HashMap::new(),
            stack: Vec::new(),
            length: 0,
            log: Vec::new(),
            pending: BTreeMap::new(),
            open_entries: 0,
            segments_live: 0,
            chain_links: 0,
            work: 0,
            work_limit,
            peak_memory: 0,
            memory_limit,
            fixed_memory,
            peak_open_entries: 0,
        }
    }

    fn spend(&mut self, amount: usize, instruction: u32) -> Result<(), LeftoverRefusal> {
        self.work = self.work.saturating_add(amount as u64);
        if self.work > self.work_limit {
            return Err(LeftoverRefusal::PastTheBound { instruction });
        }
        Ok(())
    }

    /// Holds the bytes the pass keeps against its limit: the log, the
    /// segments and chains the pending entries keep, the entries, the write
    /// times, the taint stack, and `transient` entries of a join's tables.
    fn hold(&mut self, transient: usize, instruction: u32) -> Result<(), LeftoverRefusal> {
        let records = self.log.len() as u64 + self.segments_live;
        let memory = self.fixed_memory
            + records * RECORD_BYTES
            + self.chain_links * LINK_BYTES
            + self.open_entries as u64 * ENTRY_BYTES
            + self.written.len() as u64 * TABLE_ENTRY_BYTES
            + self.stack.len() as u64 * TAINT_BYTES
            + transient as u64 * TABLE_ENTRY_BYTES;
        self.peak_memory = self.peak_memory.max(memory);
        if memory > self.memory_limit {
            return Err(LeftoverRefusal::PastTheBound { instruction });
        }
        Ok(())
    }

    fn last_taint(&self, register: u16) -> u64 {
        let position = self.stack[..self.length].partition_point(|taint| taint.end <= register);
        match position {
            0 => 0,
            position => self.stack[position - 1].time,
        }
    }

    fn written_at(&self, register: u16) -> u64 {
        self.written.get(&register).copied().unwrap_or(0)
    }

    fn tainted(&self, register: u16) -> bool {
        register >= self.local_count
            && register < self.register_count
            && self.last_taint(register) > self.written_at(register)
    }

    fn set_written(&mut self, register: u16, new: u64, cleans: bool) {
        let old = if new == 0 {
            self.written.remove(&register)
        } else {
            self.written.insert(register, new)
        }
        .unwrap_or(0);
        self.log.push(Record::Written {
            register,
            old,
            new,
            cleans,
        });
    }

    fn write(&mut self, register: u16) {
        if register < self.local_count || register >= self.register_count {
            return;
        }
        self.now += 1;
        self.set_written(register, self.now, true);
    }

    fn taint(&mut self, end: u16) {
        self.now += 1;
        let new = Taint {
            end,
            time: self.now,
        };
        let position = self.stack[..self.length].partition_point(|taint| taint.end < end);
        let old = self.stack.get(position).copied();
        let old_length = self.length as u32;
        self.place(position, new);
        self.log.push(Record::Tainted {
            position: position as u32,
            old,
            old_length,
            new,
        });
    }

    fn place(&mut self, position: usize, taint: Taint) {
        if position < self.stack.len() {
            self.stack[position] = taint;
        } else {
            self.stack.push(taint);
        }
        self.length = position + 1;
    }

    fn undo(&mut self, record: Record) {
        match record {
            Record::Written { register, old, .. } => {
                if old == 0 {
                    self.written.remove(&register);
                } else {
                    self.written.insert(register, old);
                }
            }
            Record::Tainted {
                position,
                old,
                old_length,
                ..
            } => {
                match old {
                    Some(taint) => self.stack[position as usize] = taint,
                    None => self.stack.truncate(position as usize),
                }
                self.length = old_length as usize;
            }
        }
    }

    /// Applies `record` again on the version it was made on, recording
    /// what it overwrites now, which the physical stack above its length
    /// may no longer hold.
    fn redo(&mut self, record: Record) {
        match record {
            Record::Written {
                register,
                new,
                cleans,
                ..
            } => self.set_written(register, new, cleans),
            Record::Tainted { position, new, .. } => {
                let position = position as usize;
                let old = self.stack.get(position).copied();
                let old_length = self.length as u32;
                self.place(position, new);
                self.log.push(Record::Tainted {
                    position: position as u32,
                    old,
                    old_length,
                    new,
                });
            }
        }
    }

    fn open(&mut self, target: u32, instruction: u32) -> Result<(), LeftoverRefusal> {
        self.open_entries += 1;
        self.peak_open_entries = self.peak_open_entries.max(self.open_entries);
        if self.open_entries > OPEN_ENTRY_LIMIT {
            return Err(LeftoverRefusal::PastTheBound { instruction });
        }
        let entry = Entry {
            anchor: self.log.len(),
            chain: Vec::new(),
        };
        self.pending.entry(target).or_default().push(entry);
        self.spend(1, instruction)
    }

    /// Undoes the log down to `anchor`. Every pending entry anchored above
    /// it keeps the records it still needs as a segment of its chain, one
    /// segment shared by all of them.
    fn truncate(
        &mut self,
        anchor: usize,
        arrivals: &mut [Entry],
        instruction: u32,
    ) -> Result<(), LeftoverRefusal> {
        let undone = self.log.len() - anchor;
        self.spend(undone + self.open_entries, instruction)?;
        for index in (anchor..self.log.len()).rev() {
            let record = self.log[index];
            self.undo(record);
        }
        let needed = self
            .pending
            .values()
            .flatten()
            .chain(arrivals.iter())
            .any(|entry| entry.anchor > anchor);
        if !needed {
            self.log.truncate(anchor);
            return Ok(());
        }
        let segment = Rc::new(self.log.split_off(anchor));
        self.segments_live += segment.len() as u64;
        for entry in self
            .pending
            .values_mut()
            .flatten()
            .chain(arrivals.iter_mut())
        {
            if entry.anchor > anchor {
                entry
                    .chain
                    .insert(0, (Rc::clone(&segment), 0..entry.anchor - anchor));
                entry.anchor = anchor;
                self.chain_links += 1;
            }
        }
        Ok(())
    }

    /// Drops what `entry` held once it was reached.
    fn release(&mut self, entry: Entry) {
        self.open_entries -= 1;
        for (segment, _) in &entry.chain {
            self.chain_links -= 1;
            if Rc::strong_count(segment) == 1 {
                self.segments_live -= segment.len() as u64;
            }
        }
    }

    /// Makes the current state the version `entry` carried, from an
    /// unreachable point.
    fn switch_to(
        &mut self,
        entry: &Entry,
        arrivals: &mut [Entry],
        instruction: u32,
    ) -> Result<(), LeftoverRefusal> {
        self.truncate(entry.anchor, arrivals, instruction)?;
        let redone: usize = entry.chain.iter().map(|(_, range)| range.len()).sum();
        self.spend(redone, instruction)?;
        for (segment, range) in &entry.chain {
            for record in &segment[range.clone()] {
                self.redo(*record);
            }
        }
        self.hold(0, instruction)
    }

    /// Joins the version `entry` carried into the current state: a
    /// register is tainted after the join where it is tainted in either.
    ///
    /// The two versions share the log up to the entry's anchor, the
    /// version `A`; the current one then took the log above it, the
    /// entry's its chain. A register neither touched has the same status in
    /// both. Below the lowest taint of the chain, the entry's version is
    /// `A`'s but where the chain wrote, so a register the current path
    /// wrote and the entry's version holds tainted gets its write time from
    /// `A` back. At and above it, one taint over the current state covers
    /// everything the chain tainted, and the registers both versions hold
    /// clean there, which the chain wrote, are written again.
    fn join(&mut self, entry: &Entry, instruction: u32) -> Result<(), LeftoverRefusal> {
        let anchor = entry.anchor;
        let chain_length: usize = entry.chain.iter().map(|(_, range)| range.len()).sum();
        self.spend(self.log.len() - anchor + chain_length, instruction)?;
        // The current path since `A`: each register's write time at `A`,
        // and `A`'s taint stack where the path overwrote it.
        let mut written_at_anchor: BTreeMap<u16, u64> = BTreeMap::new();
        let mut stack_at_anchor: HashMap<u32, Taint> = HashMap::new();
        let mut length_at_anchor: Option<usize> = None;
        for record in &self.log[anchor..] {
            match *record {
                Record::Written { register, old, .. } => {
                    written_at_anchor.entry(register).or_insert(old);
                }
                Record::Tainted {
                    position,
                    old,
                    old_length,
                    ..
                } => {
                    if let Some(old) = old {
                        stack_at_anchor.entry(position).or_insert(old);
                    }
                    length_at_anchor.get_or_insert(old_length as usize);
                }
            }
        }
        let length_at_anchor = length_at_anchor.unwrap_or(self.length);
        // The entry's chain, from its end: each register's status there and
        // the time it was last given, and the chain's lowest taint.
        let mut entry_status: BTreeMap<u16, (bool, u64)> = BTreeMap::new();
        let mut lowest_taint: u32 = u32::MAX;
        for (segment, range) in entry.chain.iter().rev() {
            for record in segment[range.clone()].iter().rev() {
                match *record {
                    Record::Tainted { new, .. } => {
                        lowest_taint = lowest_taint.min(u32::from(new.end));
                    }
                    Record::Written {
                        register,
                        new,
                        cleans,
                        ..
                    } => {
                        entry_status.entry(register).or_insert_with(|| {
                            let tainted = !cleans || lowest_taint <= u32::from(register);
                            (tainted, new)
                        });
                    }
                }
            }
        }
        self.hold(
            written_at_anchor.len() + stack_at_anchor.len() + entry_status.len(),
            instruction,
        )?;
        let tainted_at_anchor = |pass: &Pass, register: u16, written: u64| -> bool {
            let slot = |position: usize| {
                stack_at_anchor
                    .get(&(position as u32))
                    .copied()
                    .unwrap_or(pass.stack[position])
            };
            let (mut low, mut high) = (0usize, length_at_anchor);
            while low < high {
                let middle = low + (high - low) / 2;
                if slot(middle).end <= register {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
            let last = if low == 0 { 0 } else { slot(low - 1).time };
            last > written
        };
        // Below the chain's lowest taint.
        let mut taint_again: Vec<(u16, u64)> = Vec::new();
        for (&register, &written) in &written_at_anchor {
            if u32::from(register) >= lowest_taint || self.tainted(register) {
                continue;
            }
            let (tainted, time) = match entry_status.get(&register) {
                Some(&status) => status,
                None => (tainted_at_anchor(self, register, written), written),
            };
            if tainted {
                taint_again.push((register, time));
            }
        }
        for (&register, &(tainted, time)) in &entry_status {
            if u32::from(register) < lowest_taint
                && tainted
                && !written_at_anchor.contains_key(&register)
                && !self.tainted(register)
            {
                taint_again.push((register, time));
            }
        }
        // At and above it.
        let mut clean_in_both: Vec<u16> = Vec::new();
        if lowest_taint < u32::from(self.register_count) {
            for (&register, &(tainted, _)) in &entry_status {
                if u32::from(register) >= lowest_taint && !tainted && !self.tainted(register) {
                    clean_in_both.push(register);
                }
            }
        }
        self.spend(
            written_at_anchor.len()
                + entry_status.len() * 2
                + taint_again.len()
                + clean_in_both.len(),
            instruction,
        )?;
        for (register, time) in taint_again {
            self.set_written(register, time, false);
        }
        if lowest_taint < u32::from(self.register_count) {
            self.taint(lowest_taint as u16);
            for register in clean_in_both {
                self.write(register);
            }
        }
        self.hold(0, instruction)
    }

    /// The pass. With `stop`, answers `Some(())` with the state just
    /// before that instruction, or `None` when no path reaches it.
    fn run(
        &mut self,
        effects: &[Effect],
        instructions: Option<&[Instruction]>,
        loop_floors: &[Option<u16>],
        is_loop_head: &[bool],
        stop: Option<u32>,
    ) -> Result<Option<()>, LeftoverRefusal> {
        let count = effects.len();
        let mut reachable = count > 0;
        for index in 0..count {
            let here = index as u32;
            if let Some(mut arrivals) = self.pending.remove(&here) {
                if !reachable {
                    let chosen = arrivals
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| entry.chain.is_empty())
                        .max_by_key(|(_, entry)| entry.anchor)
                        .map_or(0, |(position, _)| position);
                    let base = arrivals.swap_remove(chosen);
                    self.switch_to(&base, &mut arrivals, here)?;
                    self.release(base);
                    reachable = true;
                }
                for entry in std::mem::take(&mut arrivals) {
                    self.join(&entry, here)?;
                    self.release(entry);
                }
            }
            if !reachable {
                if stop == Some(here) {
                    return Ok(None);
                }
                continue;
            }
            if is_loop_head[index] {
                if let Some(floor) = loop_floors[index] {
                    self.taint(floor);
                }
            }
            if stop == Some(here) {
                return Ok(Some(()));
            }
            let effect = effects[index];
            let mut cost = 1;
            if let Some(instructions) = instructions {
                for register in registers_read(&instructions[index]) {
                    cost += 1;
                    if self.tainted(register) {
                        return Err(LeftoverRefusal::Read {
                            instruction: here,
                            register,
                        });
                    }
                }
            }
            for slot in effect.slots.0..effect.slots.1 {
                self.write(slot);
            }
            cost += usize::from(effect.slots.1 - effect.slots.0);
            if let Some(end) = effect.taint {
                self.taint(end);
                cost += 1;
            }
            self.spend(cost, here)?;
            if let Some(target) = effect.target {
                // A backward jump is a loop's back edge: the head's taint
                // already stands for what it brings.
                if target > here && (target as usize) < count {
                    self.open(target, here)?;
                }
            }
            if !effect.falls_through {
                reachable = false;
                continue;
            }
            if let Some(destination) = effect.destination {
                self.write(destination);
                self.spend(1, here)?;
            }
            self.hold(0, here)?;
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests;
