//! The interrupt points' metadata (`docs/design/execution.md`, "Planned:
//! the deoptimization contract", "Optimized frames"): for every point at
//! which an observer may read optimized code's frame -- every frame node,
//! every back edge's poll -- its program counter, which of the two ways a
//! frame resumes there, and where the value of each register the leftover
//! analysis does not taint is. In stage 1 that is always its frame word:
//! the point's write-back (`allocate::DirtyPoint`) stores every binding
//! that is not there yet, and `verify_allocation` holds the allocation to
//! it by replaying every move.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the optimizing compiler's emitter, Task 5's, is the metadata's first caller"
    )
)]

use std::collections::HashMap;
use std::ops::Range;

use super::allocate::{allocate, Allocation, Location, Move, PointKind, Spot};
use super::graph::{BlockId, Graph, Node, ValueId};
use super::liveness::{block_order, intervals_knowing, poll_target, Positions};
use super::snapshot::Snapshot;
use super::verify::VerifyError;
use crate::jit::macro_assembler::Roles;

/// How a frame stopped at an interrupt point goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resumes {
    /// The value of the send at the point is delivered to its
    /// destination: a frame below a callee.
    Deliver,
    /// The instruction at the point runs again: a poll's loop head, and
    /// every other frame node, whose template hands its instruction to
    /// the interpreter when it exits.
    Reexecute,
}

/// What an observer needs of a frame stopped at one interrupt point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InterruptPoint {
    pub(crate) program_counter: u32,
    pub(crate) resumes: Resumes,
    /// Every register the leftover analysis does not taint at the point,
    /// and where its value is.
    pub(crate) locations: Vec<(u16, Location)>,
}

/// The metadata of a body's interrupt points, in the order of the
/// allocation's `dirty_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Metadata {
    pub(crate) points: Vec<InterruptPoint>,
}

/// The allocation of `graph`, `snapshot`'s, under `roles` and its
/// metadata, which a debug build verifies.
pub(crate) fn allocate_and_describe(
    snapshot: &Snapshot,
    graph: &Graph,
    roles: &Roles,
) -> (Allocation, Metadata) {
    let order = block_order(graph);
    let intervals = intervals_knowing(graph, &order, Some(&snapshot.tainted_before));
    let allocation = allocate(graph, &intervals, roles);
    let metadata = metadata(graph, &allocation, snapshot);
    debug_assert_eq!(
        verify_allocation(graph, &allocation, &metadata, snapshot),
        Ok(()),
        "internal: the allocation is refused"
    );
    (allocation, metadata)
}

/// The program counter an interrupt point stores, and how it resumes.
fn point_of(graph: &Graph, block: BlockId, kind: PointKind) -> (u32, Resumes) {
    match kind {
        PointKind::Frame { node } => {
            let Node::Frame {
                program_counter,
                instruction,
                ..
            } = &graph.block(block).nodes[node]
            else {
                panic!("internal: a frame point is a frame node");
            };
            let resumes = if instruction.argument_slots().is_some() {
                Resumes::Deliver
            } else {
                Resumes::Reexecute
            };
            (*program_counter, resumes)
        }
        PointKind::Poll => {
            let head = poll_target(graph, block).expect("internal: a poll's block jumps back");
            (graph.block(head).program_counter, Resumes::Reexecute)
        }
    }
}

fn tainted(ranges: &[Range<u16>], register: u16) -> bool {
    ranges.iter().any(|range| range.contains(&register))
}

/// The metadata of `allocation`'s interrupt points: each register the
/// leftover analysis leaves to the frame there is in its frame word, the
/// point's write-back having stored every one that was not.
pub(crate) fn metadata(graph: &Graph, allocation: &Allocation, snapshot: &Snapshot) -> Metadata {
    let register_count =
        u16::try_from(snapshot.register_count()).expect("internal: a register number fits a u16");
    Metadata {
        points: allocation
            .dirty_at
            .iter()
            .map(|point| {
                let (program_counter, resumes) = point_of(graph, point.block, point.kind);
                let tainted_here = &snapshot.tainted_before[usize::try_from(program_counter)
                    .expect("internal: a program counter fits a usize")];
                InterruptPoint {
                    program_counter,
                    resumes,
                    locations: (0..register_count)
                        .filter(|register| !tainted(tainted_here, *register))
                        .map(|register| (register, Location::FrameWord))
                        .collect(),
                }
            })
            .collect(),
    }
}

/// What a frame word holds, as the replay knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Word {
    /// What it held when the stretch began: the interpreter's.
    Original,
    Value(ValueId),
    /// Something the replay does not know.
    Stale,
}

/// The replay's state: the machine registers, and for each bytecode
/// register the value the interpreter's frame would hold -- `None` for
/// the frame word's own -- and what optimized code's frame word holds.
#[derive(Debug, Clone)]
struct Replay {
    machine: HashMap<Spot, ValueId>,
    interpreter: Vec<Option<ValueId>>,
    words: Vec<Word>,
}

impl Replay {
    fn in_sync(&self, register: usize) -> bool {
        match (self.words[register], self.interpreter[register]) {
            (Word::Original, None) => true,
            (Word::Value(held), Some(value)) => held == value,
            _ => false,
        }
    }

    fn reset(&mut self) {
        self.machine.clear();
        self.interpreter.fill(None);
        self.words.fill(Word::Original);
    }
}

/// Checks `allocation` of `graph` and `metadata` against a replay of
/// every move: every use finds its value where its definition or the last
/// move left it; every way into a block brings what the block assumes;
/// at every interrupt point, after its write-back, every register the
/// leftover analysis does not taint (`snapshot.tainted_before`) holds in
/// its frame word what the interpreter's frame would, and the metadata
/// says so; no move names a register no value is allocated to; and no
/// value is kept past a leaf call in a register the call clobbers.
pub(crate) fn verify_allocation(
    graph: &Graph,
    allocation: &Allocation,
    metadata: &Metadata,
    snapshot: &Snapshot,
) -> Result<(), VerifyError> {
    let order = &allocation.positions.order;
    let leaf_calls = intervals_knowing(graph, order, Some(&snapshot.tainted_before)).leaf_calls;
    replay(graph, allocation, metadata, snapshot, &leaf_calls)
}

/// `verify_allocation` with the leaf calls at `leaf_calls`.
fn replay(
    graph: &Graph,
    allocation: &Allocation,
    metadata: &Metadata,
    snapshot: &Snapshot,
    leaf_calls: &[u32],
) -> Result<(), VerifyError> {
    let order = &allocation.positions.order;
    let positions = Positions::of(graph, order);
    if positions != allocation.positions
        || metadata.points.len() != allocation.dirty_at.len()
        || order.first() != Some(&BlockId(0))
    {
        return Err(VerifyError::Unaligned);
    }
    let lives = intervals_knowing(graph, order, Some(&snapshot.tainted_before));
    let register_count = snapshot.register_count().max(lives.register_count);
    let survivors = &allocation.allocatable[..allocation.survive_leaf_calls];
    let mut points = allocation.dirty_at.iter().zip(&metadata.points);
    let mut replay = Replay {
        machine: HashMap::new(),
        interpreter: vec![None; register_count],
        words: vec![Word::Original; register_count],
    };
    for block in order {
        let Some(entry) = &allocation.entries[block.index()] else {
            return Err(VerifyError::Unaligned);
        };
        if block.index() == 0
            && (!entry.machine.is_empty()
                || entry
                    .bindings
                    .iter()
                    .any(|binding| *binding != super::liveness::Binding::FRAME_WORD))
        {
            return Err(VerifyError::EdgeDisagrees {
                predecessor: *block,
                successor: *block,
            });
        }
        replay.reset();
        for (register, value) in &entry.machine {
            replay.machine.insert(Spot::Machine(*register), *value);
        }
        for (register, binding) in entry.bindings.iter().enumerate() {
            replay.interpreter[register] = binding.value;
            replay.words[register] = if binding.clean {
                binding.value.map_or(Word::Original, Word::Value)
            } else {
                Word::Stale
            };
        }
        let nodes = &graph.block(*block).nodes;
        for (index, node) in nodes.iter().enumerate() {
            let use_position = positions.node_use(*block, index);
            let definition = use_position + 1;
            apply(&mut replay, allocation, use_position)?;
            if let Node::Frame { reads, .. } = node {
                let Some((point, described)) = points.next() else {
                    return Err(VerifyError::Unaligned);
                };
                if point.position != use_position
                    || point.kind != (PointKind::Frame { node: index })
                {
                    return Err(VerifyError::Unaligned);
                }
                let mut stopped = replay.clone();
                write_back(&mut stopped, &point.stores, use_position)?;
                interrupt(&stopped, graph, *block, point.kind, described, snapshot)?;
                for (register, value) in reads {
                    let register = usize::from(*register);
                    if stopped.interpreter[register] != Some(*value) || !stopped.in_sync(register) {
                        return Err(VerifyError::StaleFrameWord {
                            register: u16::try_from(register)
                                .expect("internal: a register number fits a u16"),
                            position: use_position,
                        });
                    }
                }
                replay.reset();
                apply(&mut replay, allocation, definition)?;
                continue;
            }
            for operand in node.uses() {
                in_register(&replay, allocation, operand, use_position)?;
            }
            if let Node::FrameRead { register, .. } = node {
                if !replay.in_sync(usize::from(*register)) {
                    return Err(VerifyError::StaleFrameWord {
                        register: *register,
                        position: use_position,
                    });
                }
            }
            if let Node::IntegerBinary { fallback, .. } = node {
                if graph.block(*block).exit_position(*fallback) == Some(index) {
                    enters(&replay, graph, allocation, snapshot, *block, *fallback)?;
                }
            }
            if leaf_calls.binary_search(&use_position).is_ok() {
                let clobbered: Vec<(Spot, ValueId)> = replay
                    .machine
                    .iter()
                    .filter(|(spot, _)| match spot {
                        Spot::Machine(register) => !survivors.contains(register),
                        Spot::Scratch => true,
                    })
                    .map(|(spot, value)| (*spot, *value))
                    .collect();
                for (spot, value) in clobbered {
                    replay.machine.remove(&spot);
                    let kept = matches!(
                        allocation.location_of(value, definition),
                        Location::Register(register) if Spot::Machine(register) == spot
                    );
                    if lives.live_at(value, definition) && kept {
                        return Err(VerifyError::LeafClobbered {
                            value,
                            position: use_position,
                        });
                    }
                }
            }
            if let Some(value) = node.defined() {
                match allocation.location_of(value, definition) {
                    Location::Register(register) => {
                        if !allocation.allocatable.contains(&register) {
                            return Err(VerifyError::NotAllocatable {
                                position: definition,
                            });
                        }
                        replay.machine.insert(Spot::Machine(register), value);
                    }
                    Location::FrameWord if matches!(node, Node::FrameRead { .. }) => {}
                    Location::FrameWord => {
                        return Err(VerifyError::Misplaced {
                            value,
                            position: definition,
                        })
                    }
                }
            }
            match node {
                Node::FrameRead { register, value } => {
                    let register = usize::from(*register);
                    replay.interpreter[register] = Some(*value);
                    replay.words[register] = Word::Value(*value);
                }
                Node::Copy {
                    destination_register,
                    source,
                } => replay.interpreter[usize::from(*destination_register)] = Some(*source),
                Node::IntegerBinary {
                    destination_register,
                    value,
                    ..
                } => replay.interpreter[usize::from(*destination_register)] = Some(*value),
                Node::Constant { .. } | Node::Receiver { .. } | Node::Frame { .. } => {}
            }
            apply(&mut replay, allocation, definition)?;
        }
        let end_use = positions.end_use(*block);
        apply(&mut replay, allocation, end_use)?;
        if let Some(value) = graph.block(*block).end.uses() {
            in_register(&replay, allocation, value, end_use)?;
        }
        if poll_target(graph, *block).is_some() {
            let Some((point, described)) = points.next() else {
                return Err(VerifyError::Unaligned);
            };
            if point.position != end_use || point.kind != PointKind::Poll {
                return Err(VerifyError::Unaligned);
            }
            let mut stopped = replay.clone();
            write_back(&mut stopped, &point.stores, end_use)?;
            interrupt(&stopped, graph, *block, point.kind, described, snapshot)?;
        }
        apply(&mut replay, allocation, positions.end_definition(*block))?;
        for successor in graph.block(*block).end.successors() {
            enters(&replay, graph, allocation, snapshot, *block, successor)?;
        }
    }
    if points.next().is_some() {
        return Err(VerifyError::Unaligned);
    }
    Ok(())
}

/// Checks that `value`, read in a machine register at `position`, is
/// where the allocation says.
fn in_register(
    replay: &Replay,
    allocation: &Allocation,
    value: ValueId,
    position: u32,
) -> Result<(), VerifyError> {
    match allocation.location_of(value, position) {
        Location::Register(register)
            if replay.machine.get(&Spot::Machine(register)) == Some(&value) =>
        {
            Ok(())
        }
        _ => Err(VerifyError::Misplaced { value, position }),
    }
}

/// Replays the moves at `position`.
fn apply(replay: &mut Replay, allocation: &Allocation, position: u32) -> Result<(), VerifyError> {
    let allocatable = |spot: Spot| match spot {
        Spot::Machine(register) => allocation.allocatable.contains(&register),
        Spot::Scratch => true,
    };
    for each in allocation.moves_at(position) {
        match *each {
            Move::Store {
                value,
                from,
                register,
            } => {
                if !allocatable(Spot::Machine(from)) {
                    return Err(VerifyError::NotAllocatable { position });
                }
                if replay.machine.get(&Spot::Machine(from)) != Some(&value) {
                    return Err(VerifyError::Misplaced { value, position });
                }
                replay.words[usize::from(register)] = Word::Value(value);
            }
            Move::Load {
                value,
                register,
                into,
            } => {
                if !allocatable(Spot::Machine(into)) {
                    return Err(VerifyError::NotAllocatable { position });
                }
                if replay.words[usize::from(register)] != Word::Value(value) {
                    return Err(VerifyError::StaleFrameWord { register, position });
                }
                replay.machine.insert(Spot::Machine(into), value);
            }
            Move::Transfer { value, from, into } => {
                if !allocatable(from) || !allocatable(into) {
                    return Err(VerifyError::NotAllocatable { position });
                }
                if replay.machine.get(&from) != Some(&value) {
                    return Err(VerifyError::Misplaced { value, position });
                }
                replay.machine.insert(into, value);
            }
        }
    }
    Ok(())
}

/// Replays an interrupt point's write-back.
fn write_back(
    replay: &mut Replay,
    stores: &[(u16, ValueId, Location)],
    position: u32,
) -> Result<(), VerifyError> {
    for (register, value, location) in stores {
        match location {
            Location::Register(held)
                if replay.machine.get(&Spot::Machine(*held)) == Some(value) =>
            {
                replay.words[usize::from(*register)] = Word::Value(*value);
            }
            _ => {
                return Err(VerifyError::Misplaced {
                    value: *value,
                    position,
                })
            }
        }
    }
    Ok(())
}

/// Checks the frame at an interrupt point, after its write-back: every
/// register the leftover analysis does not taint holds the interpreter's
/// value in its frame word, and the metadata says it is there.
fn interrupt(
    stopped: &Replay,
    graph: &Graph,
    block: BlockId,
    kind: PointKind,
    described: &InterruptPoint,
    snapshot: &Snapshot,
) -> Result<(), VerifyError> {
    let (program_counter, resumes) = point_of(graph, block, kind);
    if described.program_counter != program_counter || described.resumes != resumes {
        return Err(VerifyError::MetadataDisagrees { program_counter });
    }
    let tainted_here = snapshot
        .tainted_before
        .get(usize::try_from(program_counter).expect("internal: a program counter fits a usize"))
        .ok_or(VerifyError::Unaligned)?;
    let mut described_registers = described.locations.iter().peekable();
    for register in 0..stopped.words.len() {
        let number = u16::try_from(register).expect("internal: a register number fits a u16");
        if tainted(tainted_here, number) {
            continue;
        }
        if !stopped.in_sync(register) {
            return Err(VerifyError::NotWrittenBack {
                register: number,
                program_counter,
            });
        }
        if register < snapshot.register_count()
            && described_registers.next() != Some(&(number, Location::FrameWord))
        {
            return Err(VerifyError::MetadataDisagrees { program_counter });
        }
    }
    if described_registers.next().is_some() {
        return Err(VerifyError::MetadataDisagrees { program_counter });
    }
    Ok(())
}

/// Checks that control leaving `predecessor` in `replay`'s state brings
/// what `successor`'s entry assumes: each of its machine registers'
/// values -- a phi's input for the phi -- and each register's binding,
/// held in its frame word wherever the entry says so, or where it forgets
/// it, unless the leftover analysis taints the register there.
fn enters(
    replay: &Replay,
    graph: &Graph,
    allocation: &Allocation,
    snapshot: &Snapshot,
    predecessor: BlockId,
    successor: BlockId,
) -> Result<(), VerifyError> {
    let disagrees = VerifyError::EdgeDisagrees {
        predecessor,
        successor,
    };
    let Some(entry) = &allocation.entries[successor.index()] else {
        return Err(disagrees);
    };
    let block = graph.block(successor);
    let Some(way) = block
        .predecessors
        .iter()
        .position(|each| *each == predecessor)
    else {
        return Err(disagrees);
    };
    let brought = |value: ValueId| {
        block
            .phis
            .iter()
            .find(|phi| phi.value == value)
            .map_or(value, |phi| phi.inputs[way])
    };
    for (register, value) in &entry.machine {
        if replay.machine.get(&Spot::Machine(*register)) != Some(&brought(*value)) {
            return Err(disagrees);
        }
    }
    for (register, assumed) in entry.bindings.iter().enumerate() {
        let phi = block
            .phis
            .iter()
            .find(|phi| usize::from(phi.register) == register);
        match (phi, assumed.value) {
            (Some(phi), Some(value)) if value == phi.value => {
                if replay.interpreter[register] != Some(brought(value)) {
                    return Err(disagrees);
                }
            }
            (Some(_), _) => return Err(disagrees),
            (None, Some(value)) => {
                if replay.interpreter[register] != Some(value) {
                    return Err(disagrees);
                }
            }
            (None, None) => {}
        }
        if assumed.clean && !replay.in_sync(register) {
            return Err(disagrees);
        }
        if !assumed.clean && assumed.value.is_none() {
            let tainted_there = snapshot
                .tainted_before
                .get(usize::try_from(block.program_counter).unwrap_or(usize::MAX))
                .is_some_and(|ranges| {
                    u16::try_from(register).is_ok_and(|number| tainted(ranges, number))
                });
            if !tainted_there {
                return Err(disagrees);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn replay_with_leaf_calls(
    graph: &Graph,
    allocation: &Allocation,
    metadata: &Metadata,
    snapshot: &Snapshot,
    leaf_calls: &[u32],
) -> Result<(), VerifyError> {
    replay(graph, allocation, metadata, snapshot, leaf_calls)
}
