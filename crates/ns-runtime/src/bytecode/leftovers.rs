//! What a callee may leave in its sender's scratch registers: a forward
//! analysis over a body's control flow (`docs/design/compiler.md`, "What a
//! callee leaves").
//!
//! A send's callee is laid on its argument slots, so its header and window
//! go over every register of the sender above the slots, and after the
//! send returns those registers hold whatever the callee left there. Every
//! send but `SendMirror`, whose leaf's activation lands above its sender's
//! window, therefore *taints* the registers at and above its slot end. A
//! send writes none of its slots for certain -- the run loop reads one
//! argument, or consecutive ones, where they stand, and a primitive or an
//! accessor answers without a push -- so a slot keeps whatever status it
//! had. A write of a register cleans it. Registers below `local_count` --
//! the arguments, the header's numbers and the named locals -- are never
//! tainted: no callee is laid below the scratch.
//!
//! The analysis makes one pass over the instructions in order and keeps one
//! current state. A register is tainted when the last taint covering it is
//! later than its last write: each write records its time, and the taints
//! are a stack of `(end, time)` with rising ends, a later taint popping
//! every one with an end at or above its own. A time is a record's position
//! on the path from the body's start. Every change is a record in an undo
//! log, so a forward jump stores no state: it opens a pending entry
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
//! bound linear in its encoded size. None of them refuses a body either
//! lowering emits (the proof is `docs/design/compiler.md`, "What a callee
//! leaves").
//!
//! Its readers -- the decoder's refusal, a debug build's check of every
//! compiled body, the collector's pre-pass -- are still to come, so only
//! the tests call it yet.
#![cfg_attr(not(test), allow(dead_code))]

use std::rc::Rc;

use super::{falls_through, jump_target, register_written, registers_read, Instruction};

/// The parser's nesting limit (`ns_syntax`'s `MAX_DEPTH`): how many guarded
/// productions -- a primary, a block literal among them -- may be open at
/// once. Pinned by `the_parser_accepts_no_deeper_nesting`.
pub(crate) const PARSER_NESTING_LIMIT: usize = 128;

/// How many forward jumps a compiled body keeps open at once, at most.
///
/// Every forward jump the optimized lowering emits belongs to an inlined
/// form (`emit_guarded_branch`, `lower_while`, `lower_mirror_send`,
/// `lower_integer_binary`); the reference lowering emits none. Over the
/// code inside one of its block literals a form keeps at most two of its
/// own jumps open, and over its own instructions at most three: a guarded
/// branch's guard, its false branch and its true arm's exit, just as the
/// exit opens. Every form's own instructions lie inside at most
/// `PARSER_NESTING_LIMIT - 1` block literals, since its operands or its
/// blocks take one guarded production more. So at most `2 × 127 + 3` are
/// open, the most of a guarded branch at the deepest point a form can
/// stand at (`docs/design/compiler.md`, "What a callee leaves").
pub(crate) const OPEN_ENTRY_LIMIT: usize = 2 * (PARSER_NESTING_LIMIT - 1) + 3;

/// The work the analysis may spend per unit of a body's size, and beyond
/// it. A unit is an instruction or one register it reads.
pub(crate) const WORK_PER_UNIT: u64 = 14 * OPEN_ENTRY_LIMIT as u64 + 32;
pub(crate) const WORK_ALLOWANCE: u64 = 1 << 16;

/// The bytes the analysis may hold at once, per byte of a body's smallest
/// encoding, and beyond it. The smallest encoding gives an instruction
/// three bytes, a tag and one register, and two more to each register it
/// reads; the image's encoding of a body takes at least that. The
/// allowance covers the write times, one `u32` per register up to the
/// highest one written, at most 256 KiB, and the at most `OPEN_ENTRY_LIMIT`
/// pending entries.
pub(crate) const MEMORY_PER_BYTE: u64 = 96;
pub(crate) const MEMORY_ALLOWANCE: u64 = 320 << 10;

/// A register or a taint end that stands for none.
const NONE: u16 = u16::MAX;

/// A jump target that stands for none.
const NO_TARGET: u32 = u32::MAX;

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

/// What the analysis spent and held on one body, against its size.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Spent {
    pub(crate) work: u64,
    pub(crate) peak_memory: u64,
    pub(crate) peak_open_entries: usize,
    pub(crate) units: u64,
    pub(crate) bytes: u64,
}

/// What the analysis found of one body: enough to answer, for any program
/// counter, which registers may hold a callee's leftovers there.
pub(crate) struct Leftovers {
    effects: Vec<Effect>,
    loops: Vec<Region>,
    local_count: u16,
    register_count: u16,
}

impl Leftovers {
    /// The registers that may hold a callee's leftovers just before the
    /// instruction at `program_counter` runs, in ascending order; none for
    /// an instruction no path reaches. Runs the pass up to it.
    pub(crate) fn tainted_before(&self, program_counter: u32) -> impl Iterator<Item = u16> + '_ {
        self.tainted_before_each(&[program_counter])
            .pop()
            .unwrap_or_default()
            .into_iter()
    }

    /// The registers `tainted_before` answers, for every program counter of
    /// `program_counters` and in its order, from one pass: what the
    /// collector's pre-pass asks of a code once for all the frames it meets
    /// running it, and caches.
    pub(crate) fn tainted_before_each(&self, program_counters: &[u32]) -> Vec<Vec<u16>> {
        let mut stops: Vec<u32> = program_counters.to_vec();
        stops.sort_unstable();
        stops.dedup();
        let mut pass = Pass::new(self.local_count, self.register_count, u64::MAX, u64::MAX, 0);
        let mut answers = Vec::with_capacity(stops.len());
        let code = Code::Effects(&self.effects);
        // A body `analyze` accepted is accepted again.
        let _ = pass.run(&code, &self.loops, &stops, &mut answers);
        program_counters
            .iter()
            .map(|program_counter| {
                stops
                    .binary_search(program_counter)
                    .ok()
                    .and_then(|position| answers.get(position).cloned())
                    .unwrap_or_default()
            })
            .collect()
    }
}

/// Checks `instructions`, the body of a code whose first scratch register
/// is `local_count` and whose register file is `register_count` long, and
/// keeps nothing: what the image decoder runs.
pub(crate) fn check(
    instructions: &[Instruction],
    local_count: u16,
    register_count: u16,
) -> Result<Spent, LeftoverRefusal> {
    let local_count = local_count.min(register_count);
    let code = Code::Instructions {
        instructions,
        local_count,
        register_count,
    };
    let size = size_of(instructions);
    let mut pass = Pass::bounded(local_count, register_count, size, 0);
    let loops = loops(&code, &mut pass)?;
    pass.fixed_memory += (loops.capacity() * std::mem::size_of::<Region>()) as u64;
    pass.run(&code, &loops, &[], &mut Vec::new())?;
    Ok(pass.spent(size))
}

/// `check`, keeping what the pass reads of each instruction, so that the
/// tainted registers can be answered later without the instructions.
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
    let code = Code::Instructions {
        instructions,
        local_count,
        register_count,
    };
    let size = size_of(instructions);
    let mut pass = Pass::bounded(
        local_count,
        register_count,
        size,
        (effects.capacity() * std::mem::size_of::<Effect>()) as u64,
    );
    let loops = loops(&code, &mut pass)?;
    pass.fixed_memory += (loops.capacity() * std::mem::size_of::<Region>()) as u64;
    pass.run(&code, &loops, &[], &mut Vec::new())?;
    Ok(Leftovers {
        effects,
        loops,
        local_count,
        register_count,
    })
}

/// A body's units and the bytes of its smallest encoding.
fn size_of(instructions: &[Instruction]) -> (u64, u64) {
    instructions
        .iter()
        .fold((0, 0), |(units, bytes), instruction| {
            let reads = registers_read(instruction).count() as u64;
            (units + 1 + reads, bytes + 3 + 2 * reads)
        })
}

/// What one instruction does to the state: the end of the registers it
/// taints, the register it writes after its callee returned, where it may
/// jump, and whether it falls through. A write below `local_count` is left
/// out, since nothing there is ever tainted.
#[derive(Clone, Copy)]
struct Effect {
    taint: u16,
    destination: u16,
    target: u32,
    falls_through: bool,
}

impl Effect {
    fn of(instruction: &Instruction, local_count: u16, register_count: u16) -> Effect {
        // The callee is laid on the slots and its header where they end; a
        // mirror send's leaf lands above the window instead.
        let taint = match instruction {
            Instruction::SendMirror { .. } => None,
            other => other.argument_slots().map(|(base, sources)| {
                (usize::from(base) + sources.len()).max(usize::from(local_count))
            }),
        }
        .filter(|end| *end < usize::from(register_count))
        .map_or(NONE, |end| end as u16);
        Effect {
            taint,
            destination: register_written(instruction)
                .filter(|register| *register >= local_count && *register < register_count)
                .unwrap_or(NONE),
            target: jump_target(instruction).unwrap_or(NO_TARGET),
            falls_through: falls_through(instruction),
        }
    }
}

/// The body the pass runs over: the instructions, whose reads it checks,
/// or the effects `analyze` kept of them.
enum Code<'body> {
    Instructions {
        instructions: &'body [Instruction],
        local_count: u16,
        register_count: u16,
    },
    Effects(&'body [Effect]),
}

impl Code<'_> {
    fn len(&self) -> usize {
        match self {
            Code::Instructions { instructions, .. } => instructions.len(),
            Code::Effects(effects) => effects.len(),
        }
    }

    fn effect(&self, index: usize) -> Effect {
        match self {
            Code::Instructions {
                instructions,
                local_count,
                register_count,
            } => Effect::of(&instructions[index], *local_count, *register_count),
            Code::Effects(effects) => effects[index],
        }
    }

    fn instruction(&self, index: usize) -> Option<&Instruction> {
        match self {
            Code::Instructions { instructions, .. } => Some(&instructions[index]),
            Code::Effects(_) => None,
        }
    }
}

/// A loop: its head, its last back edge, and the lowest taint end of any
/// instruction between them, `NONE` for none.
#[derive(Clone, Copy)]
struct Region {
    head: u32,
    end: u32,
    floor: u16,
}

/// The loops of a body, ordered by head. Refuses loops that cross, and a
/// jump from outside a loop to anywhere in it but its head: the head's
/// taint stands for every iteration only when every path into the loop
/// passes the head.
fn loops(code: &Code<'_>, pass: &mut Pass) -> Result<Vec<Region>, LeftoverRefusal> {
    let count = code.len();
    let mut regions: Vec<Region> = Vec::new();
    let mut jumps: Vec<(u32, u32)> = Vec::new();
    for index in 0..count {
        let target = code.effect(index).target;
        if target == NO_TARGET {
            continue;
        }
        jumps.push((target, index as u32));
        if (target as usize) <= index {
            regions.push(Region {
                head: target,
                end: index as u32,
                floor: NONE,
            });
        }
    }
    pass.spend(count + jumps.len() * 2, 0)?;
    regions.sort_unstable_by_key(|region| (region.head, std::cmp::Reverse(region.end)));
    regions.dedup_by_key(|region| region.head);
    jumps.sort_unstable();
    let transient = jumps.capacity() * std::mem::size_of::<(u32, u32)>()
        + regions.capacity() * std::mem::size_of::<Region>() * 2;
    pass.hold_transient(transient as u64, 0)?;
    // Properly nested, and each one's floor.
    let mut open: Vec<usize> = Vec::new();
    let mut next = 0;
    for index in 0..count {
        while let Some(&innermost) = open.last() {
            if regions[innermost].end as usize >= index {
                break;
            }
            open.pop();
            if let Some(&parent) = open.last() {
                regions[parent].floor = regions[parent].floor.min(regions[innermost].floor);
            }
        }
        while next < regions.len() && regions[next].head as usize == index {
            if let Some(&parent) = open.last() {
                if regions[next].end > regions[parent].end {
                    return Err(LeftoverRefusal::PastTheBound {
                        instruction: regions[next].end,
                    });
                }
            }
            open.push(next);
            next += 1;
        }
        if let Some(&innermost) = open.last() {
            regions[innermost].floor = regions[innermost].floor.min(code.effect(index).taint);
        }
    }
    while let Some(innermost) = open.pop() {
        if let Some(&parent) = open.last() {
            regions[parent].floor = regions[parent].floor.min(regions[innermost].floor);
        }
    }
    // Entered at the head alone: a jump to a point strictly inside a loop
    // comes from inside it.
    let mut next = 0;
    for &(target, source) in &jumps {
        while next < regions.len() && regions[next].head < target {
            while let Some(&innermost) = open.last() {
                if regions[innermost].end >= regions[next].head {
                    break;
                }
                open.pop();
            }
            open.push(next);
            next += 1;
        }
        while let Some(&innermost) = open.last() {
            if regions[innermost].end >= target {
                break;
            }
            open.pop();
        }
        if let Some(&innermost) = open.last() {
            let region = regions[innermost];
            if source < region.head || source > region.end {
                return Err(LeftoverRefusal::PastTheBound {
                    instruction: source,
                });
            }
        }
    }
    pass.spend(count + jumps.len() + regions.len() * 2, 0)?;
    Ok(regions)
}

/// One taint: every register at or above `end` is tainted at `time`.
#[derive(Clone, Copy)]
struct Taint {
    end: u16,
    time: u32,
}

/// One change to the state, sixteen bytes, with what undoes and what
/// redoes it. A write time's record has `new_end` `NONE`: `subject` is the
/// register, `old_end` 1 for a write and 0 for a join putting back an older
/// time, which taints it. A taint's record has `subject` the stack slot it
/// took, `length` the stack's previous length, `old_end` and `old_time` the
/// slot's previous content (`old_end` `NONE` for a slot past the stack's
/// end), and `new_end` and `new_time` the taint.
#[derive(Clone, Copy)]
struct Record {
    subject: u16,
    length: u16,
    old_end: u16,
    new_end: u16,
    old_time: u32,
    new_time: u32,
}

impl Record {
    fn written(register: u16, old: u32, new: u32, cleans: bool) -> Record {
        Record {
            subject: register,
            length: 0,
            old_end: u16::from(cleans),
            new_end: NONE,
            old_time: old,
            new_time: new,
        }
    }

    fn is_written(&self) -> bool {
        self.new_end == NONE
    }

    fn cleans(&self) -> bool {
        self.old_end == 1
    }
}

const RECORD_BYTES: u64 = std::mem::size_of::<Record>() as u64;

/// A run of records of a shared segment, `start .. end`.
struct Link {
    segment: Rc<Vec<Record>>,
    start: u32,
    end: u32,
}

/// A forward jump not yet reached: the version it carried is the path up
/// to position `anchor`, then the records of `chain`, which the current
/// path has since left. The chain is kept last link first, so that a
/// truncation adds its link at the back.
struct Pending {
    target: u32,
    anchor: u32,
    chain: Vec<Link>,
}

impl Pending {
    fn records(&self) -> impl DoubleEndedIterator<Item = &Record> {
        self.chain
            .iter()
            .rev()
            .flat_map(|link| link.segment[link.start as usize..link.end as usize].iter())
    }

    fn chain_length(&self) -> usize {
        self.chain
            .iter()
            .map(|link| (link.end - link.start) as usize)
            .sum()
    }
}

struct Pass {
    local_count: u16,
    register_count: u16,
    /// Each register's write time, indexed by register, as long as the
    /// highest one written.
    written: Vec<u32>,
    stack: Vec<Taint>,
    length: usize,
    log: Vec<Record>,
    /// The position of the log's first record on the path: the records
    /// before it no pending entry can need.
    log_base: u32,
    pending: Vec<Pending>,
    next_target: u32,
    segment_bytes: u64,
    link_bytes: u64,
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
            written: Vec::new(),
            stack: Vec::new(),
            length: 0,
            log: Vec::new(),
            log_base: 0,
            pending: Vec::new(),
            next_target: NO_TARGET,
            segment_bytes: 0,
            link_bytes: 0,
            work: 0,
            work_limit,
            peak_memory: 0,
            memory_limit,
            fixed_memory,
            peak_open_entries: 0,
        }
    }

    fn bounded(
        local_count: u16,
        register_count: u16,
        (units, bytes): (u64, u64),
        fixed_memory: u64,
    ) -> Pass {
        Pass::new(
            local_count,
            register_count,
            WORK_PER_UNIT
                .saturating_mul(units)
                .saturating_add(WORK_ALLOWANCE),
            MEMORY_PER_BYTE
                .saturating_mul(bytes)
                .saturating_add(MEMORY_ALLOWANCE),
            fixed_memory,
        )
    }

    fn spent(&self, (units, bytes): (u64, u64)) -> Spent {
        Spent {
            work: self.work,
            peak_memory: self.peak_memory,
            peak_open_entries: self.peak_open_entries,
            units,
            bytes,
        }
    }

    fn spend(&mut self, amount: usize, instruction: u32) -> Result<(), LeftoverRefusal> {
        self.work = self.work.saturating_add(amount as u64);
        if self.work > self.work_limit {
            return Err(LeftoverRefusal::PastTheBound { instruction });
        }
        Ok(())
    }

    /// Holds the bytes the pass keeps, and `transient` bytes beside them,
    /// against its limit: every table at its capacity.
    fn hold_transient(&mut self, transient: u64, instruction: u32) -> Result<(), LeftoverRefusal> {
        let memory = self.fixed_memory
            + transient
            + self.log.capacity() as u64 * RECORD_BYTES
            + self.segment_bytes
            + self.link_bytes
            + (self.pending.capacity() * std::mem::size_of::<Pending>()) as u64
            + (self.written.capacity() * std::mem::size_of::<u32>()) as u64
            + (self.stack.capacity() * std::mem::size_of::<Taint>()) as u64;
        self.peak_memory = self.peak_memory.max(memory);
        if memory > self.memory_limit {
            return Err(LeftoverRefusal::PastTheBound { instruction });
        }
        Ok(())
    }

    fn hold(&mut self, instruction: u32) -> Result<(), LeftoverRefusal> {
        self.hold_transient(0, instruction)
    }

    /// The time of the next record: its position on the path.
    fn next_time(&self, instruction: u32) -> Result<u32, LeftoverRefusal> {
        u32::try_from(u64::from(self.log_base) + self.log.len() as u64 + 1)
            .map_err(|_| LeftoverRefusal::PastTheBound { instruction })
    }

    fn position(&self) -> u32 {
        self.log_base + self.log.len() as u32
    }

    fn last_taint(&self, register: u16) -> u32 {
        let position = self.stack[..self.length].partition_point(|taint| taint.end <= register);
        match position {
            0 => 0,
            position => self.stack[position - 1].time,
        }
    }

    fn written_at(&self, register: u16) -> u32 {
        self.written
            .get(usize::from(register))
            .copied()
            .unwrap_or(0)
    }

    fn tainted(&self, register: u16) -> bool {
        register >= self.local_count
            && register < self.register_count
            && self.last_taint(register) > self.written_at(register)
    }

    fn tainted_registers(&self) -> Vec<u16> {
        (self.local_count..self.register_count)
            .filter(|register| self.tainted(*register))
            .collect()
    }

    fn put_written(&mut self, register: u16, time: u32) {
        let index = usize::from(register);
        if index >= self.written.len() {
            if time == 0 {
                return;
            }
            self.written.resize(index + 1, 0);
        }
        self.written[index] = time;
    }

    fn set_written(&mut self, register: u16, new: u32, cleans: bool) {
        let old = self.written_at(register);
        self.put_written(register, new);
        self.log.push(Record::written(register, old, new, cleans));
    }

    fn write(&mut self, register: u16, instruction: u32) -> Result<(), LeftoverRefusal> {
        if register < self.local_count || register >= self.register_count {
            return Ok(());
        }
        let time = self.next_time(instruction)?;
        self.set_written(register, time, true);
        Ok(())
    }

    fn taint(&mut self, end: u16, instruction: u32) -> Result<(), LeftoverRefusal> {
        let new = Taint {
            end,
            time: self.next_time(instruction)?,
        };
        let position = self.stack[..self.length].partition_point(|taint| taint.end < end);
        let record = self.place(position, new);
        self.log.push(record);
        Ok(())
    }

    /// Puts `taint` in the stack's slot `position`, popping everything from
    /// there up, and answers the record that undoes it.
    fn place(&mut self, position: usize, taint: Taint) -> Record {
        let old = self.stack.get(position).copied();
        let record = Record {
            subject: position as u16,
            length: self.length as u16,
            old_end: old.map_or(NONE, |old| old.end),
            new_end: taint.end,
            old_time: old.map_or(0, |old| old.time),
            new_time: taint.time,
        };
        if position < self.stack.len() {
            self.stack[position] = taint;
        } else {
            self.stack.push(taint);
        }
        self.length = position + 1;
        record
    }

    fn undo(&mut self, record: Record) {
        if record.is_written() {
            self.put_written(record.subject, record.old_time);
            return;
        }
        let position = usize::from(record.subject);
        if record.old_end == NONE {
            self.stack.truncate(position);
        } else {
            self.stack[position] = Taint {
                end: record.old_end,
                time: record.old_time,
            };
        }
        self.length = usize::from(record.length);
    }

    /// Applies `record` again on the version it was made on, recording
    /// what it overwrites now, which the stack's slots past its length may
    /// no longer hold.
    fn redo(&mut self, record: Record) {
        if record.is_written() {
            self.set_written(record.subject, record.new_time, record.cleans());
            return;
        }
        let record = self.place(
            usize::from(record.subject),
            Taint {
                end: record.new_end,
                time: record.new_time,
            },
        );
        self.log.push(record);
    }

    fn open(&mut self, target: u32, instruction: u32) -> Result<(), LeftoverRefusal> {
        if self.pending.len() >= OPEN_ENTRY_LIMIT {
            return Err(LeftoverRefusal::PastTheBound { instruction });
        }
        self.pending.push(Pending {
            target,
            anchor: self.position(),
            chain: Vec::new(),
        });
        self.peak_open_entries = self.peak_open_entries.max(self.pending.len());
        self.next_target = self.next_target.min(target);
        self.spend(1, instruction)
    }

    /// Undoes the log down to position `anchor`. Every pending entry
    /// anchored above it keeps the records it still needs as a link to one
    /// segment shared by all of them.
    fn truncate(
        &mut self,
        anchor: u32,
        arrivals: &mut [Pending],
        instruction: u32,
    ) -> Result<(), LeftoverRefusal> {
        let start = (anchor - self.log_base) as usize;
        let entries = self.pending.len() + arrivals.len();
        self.spend(self.log.len() - start + entries, instruction)?;
        for index in (start..self.log.len()).rev() {
            let record = self.log[index];
            self.undo(record);
        }
        let needed = self
            .pending
            .iter()
            .chain(arrivals.iter())
            .any(|entry| entry.anchor > anchor);
        if !needed {
            self.log.truncate(start);
            return Ok(());
        }
        let segment = Rc::new(self.log.split_off(start));
        self.segment_bytes += segment.capacity() as u64 * RECORD_BYTES;
        let mut link_bytes = 0u64;
        for entry in self.pending.iter_mut().chain(arrivals.iter_mut()) {
            if entry.anchor > anchor {
                let before = entry.chain.capacity();
                entry.chain.push(Link {
                    segment: Rc::clone(&segment),
                    start: 0,
                    end: entry.anchor - anchor,
                });
                link_bytes +=
                    ((entry.chain.capacity() - before) * std::mem::size_of::<Link>()) as u64;
                entry.anchor = anchor;
            }
        }
        self.link_bytes += link_bytes;
        Ok(())
    }

    /// Drops what `entry` held once it was reached.
    fn release(&mut self, entry: Pending, instruction: u32) -> Result<(), LeftoverRefusal> {
        self.spend(entry.chain.len(), instruction)?;
        self.link_bytes -= (entry.chain.capacity() * std::mem::size_of::<Link>()) as u64;
        for link in &entry.chain {
            if Rc::strong_count(&link.segment) == 1 {
                self.segment_bytes -= link.segment.capacity() as u64 * RECORD_BYTES;
            }
        }
        Ok(())
    }

    /// Makes the current state the version `entry` carried, from an
    /// unreachable point.
    fn switch_to(
        &mut self,
        entry: &Pending,
        arrivals: &mut [Pending],
        instruction: u32,
    ) -> Result<(), LeftoverRefusal> {
        self.truncate(entry.anchor, arrivals, instruction)?;
        self.spend(entry.chain_length(), instruction)?;
        for record in entry.records() {
            self.redo(*record);
        }
        self.hold(instruction)
    }

    /// Joins the version `entry` carried into the current state: a
    /// register is tainted after the join where it is tainted in either.
    ///
    /// The two versions share the path up to the entry's anchor, the
    /// version `A`; the current one then took the log above it, the
    /// entry's its chain. A register neither touched has the same status in
    /// both. Below the chain's lowest taint, the entry's version is `A`'s
    /// but where the chain wrote, so a register the current path wrote and
    /// the entry's version holds tainted gets its write time from `A` back.
    /// At and above it, one taint over the current state covers everything
    /// the chain tainted, and the registers both versions hold clean there,
    /// which the chain wrote, are written again.
    fn join(&mut self, entry: &Pending, instruction: u32) -> Result<(), LeftoverRefusal> {
        let start = (entry.anchor - self.log_base) as usize;
        self.spend(self.log.len() - start + entry.chain_length(), instruction)?;
        // The current path since `A`: each register's write time at `A`,
        // and `A`'s taint stack where the path overwrote it.
        let mut written_at_anchor: Vec<(u16, u32)> = Vec::new();
        let mut stack_at_anchor: Vec<(u16, Taint)> = Vec::new();
        let mut length_at_anchor: Option<usize> = None;
        for record in &self.log[start..] {
            if record.is_written() {
                written_at_anchor.push((record.subject, record.old_time));
            } else {
                if record.old_end != NONE {
                    stack_at_anchor.push((
                        record.subject,
                        Taint {
                            end: record.old_end,
                            time: record.old_time,
                        },
                    ));
                }
                length_at_anchor.get_or_insert(usize::from(record.length));
            }
        }
        // The first record of each register and slot holds its value at `A`.
        written_at_anchor.sort_by_key(|(register, _)| *register);
        written_at_anchor.dedup_by_key(|(register, _)| *register);
        stack_at_anchor.sort_by_key(|(position, _)| *position);
        stack_at_anchor.dedup_by_key(|(position, _)| *position);
        let length_at_anchor = length_at_anchor.unwrap_or(self.length);
        // The entry's chain, from its end: each register's status there and
        // the time it was last given, and the chain's lowest taint.
        let mut entry_status: Vec<(u16, bool, u32)> = Vec::new();
        let mut lowest_taint: u32 = u32::MAX;
        for record in entry.records().rev() {
            if record.is_written() {
                let tainted = !record.cleans() || lowest_taint <= u32::from(record.subject);
                entry_status.push((record.subject, tainted, record.new_time));
            } else {
                lowest_taint = lowest_taint.min(u32::from(record.new_end));
            }
        }
        // Read from the end, the first record of each register is its last.
        entry_status.sort_by_key(|(register, _, _)| *register);
        entry_status.dedup_by_key(|(register, _, _)| *register);
        let tables = written_at_anchor.capacity() * std::mem::size_of::<(u16, u32)>()
            + stack_at_anchor.capacity() * std::mem::size_of::<(u16, Taint)>()
            + entry_status.capacity() * std::mem::size_of::<(u16, bool, u32)>();
        self.hold_transient(tables as u64, instruction)?;
        let status_in_entry = |register: u16| {
            entry_status
                .binary_search_by_key(&register, |(entry_register, _, _)| *entry_register)
                .ok()
                .map(|position| (entry_status[position].1, entry_status[position].2))
        };
        let tainted_at_anchor = |pass: &Pass, register: u16, written: u32| -> bool {
            let slot = |position: usize| {
                stack_at_anchor
                    .binary_search_by_key(&(position as u16), |(slot, _)| *slot)
                    .map_or(pass.stack[position], |found| stack_at_anchor[found].1)
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
        let mut taint_again: Vec<(u16, u32)> = Vec::new();
        for &(register, written) in &written_at_anchor {
            if u32::from(register) >= lowest_taint || self.tainted(register) {
                continue;
            }
            let (tainted, time) = status_in_entry(register)
                .unwrap_or_else(|| (tainted_at_anchor(self, register, written), written));
            if tainted {
                taint_again.push((register, time));
            }
        }
        for &(register, tainted, time) in &entry_status {
            if u32::from(register) < lowest_taint
                && tainted
                && written_at_anchor
                    .binary_search_by_key(&register, |(written_register, _)| *written_register)
                    .is_err()
                && !self.tainted(register)
            {
                taint_again.push((register, time));
            }
        }
        // At and above it.
        let mut clean_in_both: Vec<u16> = Vec::new();
        if lowest_taint < u32::from(self.register_count) {
            for &(register, tainted, _) in &entry_status {
                if u32::from(register) >= lowest_taint && !tainted && !self.tainted(register) {
                    clean_in_both.push(register);
                }
            }
        }
        self.spend(
            written_at_anchor.len()
                + stack_at_anchor.len()
                + entry_status.len() * 2
                + taint_again.len()
                + clean_in_both.len(),
            instruction,
        )?;
        let extra = (taint_again.capacity() * std::mem::size_of::<(u16, u32)>()
            + clean_in_both.capacity() * std::mem::size_of::<u16>()) as u64;
        self.hold_transient(tables as u64 + extra, instruction)?;
        drop(written_at_anchor);
        drop(stack_at_anchor);
        drop(entry_status);
        for (register, time) in taint_again {
            self.set_written(register, time, false);
        }
        if lowest_taint < u32::from(self.register_count) {
            self.taint(lowest_taint as u16, instruction)?;
            for register in clean_in_both {
                self.write(register, instruction)?;
            }
        }
        self.hold(instruction)
    }

    /// The pending entries targeting `here`, out of the table.
    fn arrivals(&mut self, here: u32) -> Result<Vec<Pending>, LeftoverRefusal> {
        self.spend(self.pending.len(), here)?;
        let mut arrivals = Vec::new();
        let mut index = 0;
        while index < self.pending.len() {
            if self.pending[index].target == here {
                arrivals.push(self.pending.swap_remove(index));
            } else {
                index += 1;
            }
        }
        self.next_target = self
            .pending
            .iter()
            .map(|entry| entry.target)
            .min()
            .unwrap_or(NO_TARGET);
        Ok(arrivals)
    }

    /// The pass. Answers into `answers` the registers tainted just before
    /// each program counter of `stops`, which is ascending.
    fn run(
        &mut self,
        code: &Code<'_>,
        loops: &[Region],
        stops: &[u32],
        answers: &mut Vec<Vec<u16>>,
    ) -> Result<(), LeftoverRefusal> {
        let count = code.len();
        let mut reachable = count > 0;
        let mut next_loop = 0;
        let mut next_stop = 0;
        // A debug build checks every back edge against its head.
        let mut heads: Vec<(u32, Vec<u16>)> = Vec::new();
        for index in 0..count {
            let here = index as u32;
            if here == self.next_target {
                let mut arrivals = self.arrivals(here)?;
                if !reachable && !arrivals.is_empty() {
                    let chosen = arrivals
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| entry.chain.is_empty())
                        .max_by_key(|(_, entry)| entry.anchor)
                        .map_or(0, |(position, _)| position);
                    let base = arrivals.swap_remove(chosen);
                    self.switch_to(&base, &mut arrivals, here)?;
                    self.release(base, here)?;
                    reachable = true;
                }
                for entry in std::mem::take(&mut arrivals) {
                    self.join(&entry, here)?;
                    self.release(entry, here)?;
                }
            }
            while next_loop < loops.len() && loops[next_loop].head < here {
                next_loop += 1;
            }
            if reachable && next_loop < loops.len() && loops[next_loop].head == here {
                let floor = loops[next_loop].floor;
                if floor != NONE {
                    self.taint(floor, here)?;
                }
                if cfg!(debug_assertions) {
                    heads.push((here, self.tainted_registers()));
                }
            }
            if next_stop < stops.len() && stops[next_stop] == here {
                answers.push(if reachable {
                    self.tainted_registers()
                } else {
                    Vec::new()
                });
                next_stop += 1;
                if next_stop == stops.len() {
                    return Ok(());
                }
            }
            if !reachable {
                continue;
            }
            let effect = code.effect(index);
            let mut cost = 1;
            if let Some(instruction) = code.instruction(index) {
                for register in registers_read(instruction) {
                    cost += 1;
                    if self.tainted(register) {
                        return Err(LeftoverRefusal::Read {
                            instruction: here,
                            register,
                        });
                    }
                }
            }
            if effect.taint != NONE {
                self.taint(effect.taint, here)?;
            }
            self.spend(cost, here)?;
            if effect.target != NO_TARGET {
                if effect.target > here && (effect.target as usize) < count {
                    self.open(effect.target, here)?;
                } else if cfg!(debug_assertions) && effect.target <= here {
                    self.check_back_edge(&heads, effect.target);
                }
            }
            if !effect.falls_through {
                reachable = false;
                continue;
            }
            if effect.destination != NONE {
                self.write(effect.destination, here)?;
                self.spend(1, here)?;
            }
            // No entry can reach below its anchor, so with none pending the
            // log is needed no more.
            if self.pending.is_empty() {
                self.log_base += self.log.len() as u32;
                self.log.clear();
            }
            self.hold(here)?;
        }
        while next_stop < stops.len() {
            answers.push(Vec::new());
            next_stop += 1;
        }
        Ok(())
    }

    /// A back edge's state is within its head's: what the head's taint
    /// stands for.
    fn check_back_edge(&self, heads: &[(u32, Vec<u16>)], head: u32) {
        let Ok(found) = heads.binary_search_by_key(&head, |(position, _)| *position) else {
            return;
        };
        let assumed = &heads[found].1;
        for register in self.tainted_registers() {
            assert!(
                assumed.binary_search(&register).is_ok(),
                "internal: r{register} is tainted at the back edge to {head} but not at the head"
            );
        }
    }
}

#[cfg(test)]
mod tests;
