//! The interrupt points' metadata (`docs/design/execution.md`, "Planned:
//! the deoptimization contract", "Optimized frames"): for every point at
//! which an observer may read optimized code's frame -- every frame node,
//! every back edge's poll, every trap -- the chain of scopes it stops
//! in, innermost first, and for each its program counter, which of the
//! two ways it resumes there, and where the value of each of its
//! registers the leftover analysis does not taint is. That is always its
//! frame word: the point's write-back (`allocate::DirtyPoint`) stores
//! every binding that is not there yet, and `verify_allocation` holds the
//! allocation to it by replaying every move.

use std::collections::HashMap;
use std::ops::Range;

use super::allocate::{allocate, Allocation, Location, Move, PointKind, Spot};
use super::graph::{BlockId, Graph, Node, ScopeId, ValueId};
use super::liveness::{block_order, intervals_knowing, poll_target, Positions};
use super::verify::VerifyError;
use crate::heap::CodeId;
use crate::jit::macro_assembler::Roles;

/// How a frame stopped at an interrupt point goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resumes {
    /// The value of the send at the point is delivered to its
    /// destination: the resumption of a frame below its callee. It is not
    /// the only one at a send: a template that exits before its call --
    /// a call refused, a stub that hands the send to the interpreter --
    /// leaves the frame to re-execute the instruction at the same program
    /// counter, with the same frame words, so the point's locations hold
    /// for either way. Every scope an interrupt point's scope is inlined
    /// into resumes so, at the send it is inlined at.
    Deliver,
    /// The instruction at the point runs again: a poll's loop head, a
    /// trap's instruction, and every other frame node, whose template
    /// hands its instruction to the interpreter when it exits.
    Reexecute,
}

/// One scope an interrupt point stops in: the activation the exit leaves
/// of it, lying at `offset` in the physical frame -- its body, the
/// instruction it stopped at and how it goes on there, and where each of
/// its registers the leftover analysis does not taint is, numbered from
/// its register 0. An inlined scope's registers end where the scope
/// inlined into it begins, the slots of the send it is inlined at, and
/// the header words the exit writes are not among them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopeAtPoint {
    pub(crate) scope: ScopeId,
    pub(crate) code_id: CodeId,
    pub(crate) program_counter: u32,
    pub(crate) resumes: Resumes,
    pub(crate) offset: u16,
    pub(crate) locations: Vec<(u16, Location)>,
}

/// What an observer needs of a frame stopped at one interrupt point: the
/// chain of scopes it stops in, innermost first, the root last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InterruptPoint {
    pub(crate) scopes: Vec<ScopeAtPoint>,
}

/// The metadata of a body's interrupt points, in the order of the
/// allocation's `dirty_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Metadata {
    pub(crate) points: Vec<InterruptPoint>,
}

/// The allocation of `graph` under `roles` and its metadata, which a
/// debug build verifies.
pub(crate) fn allocate_and_describe(graph: &Graph, roles: &Roles) -> (Allocation, Metadata) {
    let order = block_order(graph);
    let intervals = intervals_knowing(graph, &order);
    let allocation = allocate(graph, &intervals, roles);
    let metadata = metadata(graph, &allocation);
    debug_assert_eq!(
        verify_allocation(graph, &allocation, &metadata),
        Ok(()),
        "internal: the allocation is refused"
    );
    (allocation, metadata)
}

/// The scope an interrupt point stops in, the program counter it stores
/// and how it resumes.
fn point_of(graph: &Graph, block: BlockId, kind: PointKind) -> (ScopeId, u32, Resumes) {
    let scope = graph.block(block).scope;
    match kind {
        PointKind::Frame { node } => {
            let Node::Frame {
                scope,
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
            (*scope, *program_counter, resumes)
        }
        // The loop's head runs again: where its block begins an inlined
        // scope's entry, the send that scope is inlined at, whose frame
        // the poll's exit leaves before the callee's is laid.
        PointKind::Poll => {
            let head = graph
                .block(poll_target(graph, block).expect("internal: a poll's block jumps back"));
            let (scope, program_counter) = head.taint_point;
            (scope, program_counter, Resumes::Reexecute)
        }
        // A trap runs its instruction again in the interpreter.
        PointKind::Trap { node: Some(node) } => {
            let program_counter = graph.block(block).nodes[node]
                .trap()
                .expect("internal: a trap point's node traps");
            (scope, program_counter, Resumes::Reexecute)
        }
        PointKind::Trap { node: None } => {
            let program_counter = graph
                .end_trap(block)
                .expect("internal: a trap point's end traps");
            (scope, program_counter, Resumes::Reexecute)
        }
    }
}

fn tainted(ranges: &[Range<u16>], register: u16) -> bool {
    ranges.iter().any(|range| range.contains(&register))
}

/// The chain of scopes a point that stops in `scope` at `program_counter`
/// and resumes as `resumes` describes (`ScopeAtPoint`): each register of
/// each scope the leftover analysis leaves to the frame there is in its
/// frame word, the point's write-back having stored every one that was
/// not.
fn chain_at(
    graph: &Graph,
    scope: ScopeId,
    program_counter: u32,
    resumes: Resumes,
) -> Vec<ScopeAtPoint> {
    let tainted_here = graph.tainted_at(scope, program_counter);
    let mut chain = Vec::new();
    let mut stopped = (scope, program_counter, resumes);
    let mut end = graph.scope(scope).top();
    loop {
        let (id, program_counter, resumes) = stopped;
        let owner = graph.scope(id);
        chain.push(ScopeAtPoint {
            scope: id,
            code_id: owner.code_id,
            program_counter,
            resumes,
            offset: owner.offset,
            locations: (owner.offset..end)
                .filter(|register| !tainted(&tainted_here, *register))
                .map(|register| (register - owner.offset, Location::FrameWord))
                .collect(),
        });
        let Some((sender, call)) = owner.parent else {
            return chain;
        };
        end = owner.offset;
        stopped = (sender, call, Resumes::Deliver);
    }
}

/// The metadata of `allocation`'s interrupt points (`chain_at`).
pub(crate) fn metadata(graph: &Graph, allocation: &Allocation) -> Metadata {
    Metadata {
        points: allocation
            .dirty_at
            .iter()
            .map(|point| {
                let (scope, program_counter, resumes) = point_of(graph, point.block, point.kind);
                InterruptPoint {
                    scopes: chain_at(graph, scope, program_counter, resumes),
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
/// at every interrupt point, after its write-back, every physical register
/// the leftover analysis does not taint (`Graph::tainted_at`) -- an
/// inlined scope's receiver and current class among them -- holds in its
/// frame word what the interpreter's frames would, and the metadata says
/// so of every scope the point stops in; no move names a register no
/// value is allocated to; and no value is kept past a leaf call in a
/// register the call clobbers.
pub(crate) fn verify_allocation(
    graph: &Graph,
    allocation: &Allocation,
    metadata: &Metadata,
) -> Result<(), VerifyError> {
    let order = &allocation.positions.order;
    let leaf_calls = intervals_knowing(graph, order).leaf_calls;
    replay(graph, allocation, metadata, &leaf_calls)
}

/// `verify_allocation` with the leaf calls at `leaf_calls`.
fn replay(
    graph: &Graph,
    allocation: &Allocation,
    metadata: &Metadata,
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
    let lives = intervals_knowing(graph, order);
    let named_registers = graph
        .scopes
        .iter()
        .map(|scope| usize::from(scope.top()))
        .max()
        .unwrap_or(0);
    let register_count = named_registers.max(lives.register_count);
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
                interrupt(&stopped, graph, *block, point.kind, described)?;
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
            if let Node::IntegerBinary {
                fallback: Some(fallback),
                ..
            } = node
            {
                if graph.block(*block).exit_position(*fallback) == Some(index) {
                    enters(&replay, graph, allocation, *block, *fallback)?;
                }
            }
            // The trap's cold write-back, replayed on its way out.
            if node.trap().is_some() {
                let Some((point, described)) = points.next() else {
                    return Err(VerifyError::Unaligned);
                };
                if point.position != use_position
                    || point.kind != (PointKind::Trap { node: Some(index) })
                {
                    return Err(VerifyError::Unaligned);
                }
                let mut stopped = replay.clone();
                write_back(&mut stopped, &point.stores, use_position)?;
                interrupt(&stopped, graph, *block, point.kind, described)?;
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
                    Location::FrameWord | Location::EliminatedBlock(_) => {
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
                // An inlined callee's frame is laid over the registers, or
                // left there: neither frame's value is one the replay
                // knows, and the leftover analysis taints each there.
                Node::Overlay { ranges } => {
                    for range in ranges {
                        let end = usize::from(range.end).min(replay.words.len());
                        for register in usize::from(range.start)..end {
                            replay.interpreter[register] = None;
                            replay.words[register] = Word::Stale;
                        }
                    }
                }
                Node::Constant { .. }
                | Node::Receiver { .. }
                | Node::OwnSlotLoad { .. }
                | Node::CurrentClass { .. }
                | Node::Frame { .. } => {}
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
            interrupt(&stopped, graph, *block, point.kind, described)?;
        }
        if graph.end_trap(*block).is_some() {
            let Some((point, described)) = points.next() else {
                return Err(VerifyError::Unaligned);
            };
            if point.position != end_use || point.kind != (PointKind::Trap { node: None }) {
                return Err(VerifyError::Unaligned);
            }
            let mut stopped = replay.clone();
            write_back(&mut stopped, &point.stores, end_use)?;
            interrupt(&stopped, graph, *block, point.kind, described)?;
        }
        apply(&mut replay, allocation, positions.end_definition(*block))?;
        for successor in graph.block(*block).end.successors() {
            enters(&replay, graph, allocation, *block, successor)?;
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
/// physical register the leftover analysis does not taint holds the
/// interpreter's value in its frame word, and the metadata names the
/// point's chain of scopes, each with every one of its registers among
/// them in its frame word (`chain_at`).
fn interrupt(
    stopped: &Replay,
    graph: &Graph,
    block: BlockId,
    kind: PointKind,
    described: &InterruptPoint,
) -> Result<(), VerifyError> {
    let (scope, program_counter, resumes) = stopped_at(graph, block, kind)?;
    if usize::try_from(program_counter)
        .ok()
        .is_none_or(|index| index >= graph.scope(scope).tainted_before.len())
    {
        return Err(VerifyError::Unaligned);
    }
    let tainted_here = graph.tainted_at(scope, program_counter);
    described_chain(
        graph,
        described,
        (scope, program_counter, resumes),
        &tainted_here,
    )?;
    for register in 0..stopped.words.len() {
        let number = u16::try_from(register).expect("internal: a register number fits a u16");
        if tainted(&tainted_here, number) {
            continue;
        }
        if !stopped.in_sync(register) {
            return Err(VerifyError::NotWrittenBack {
                register: number,
                program_counter,
            });
        }
    }
    Ok(())
}

/// Where a frame stopped at an interrupt point stands, found from the
/// graph's blocks and nodes alone, apart from the metadata's own reading
/// (`point_of`): the scope of the point's block -- the frame laid there --
/// at the frame node's or the trap's instruction, or, for a poll, at the
/// loop head's, which is the send its scope is inlined at in that block's
/// scope where the head begins an inlined scope.
fn stopped_at(
    graph: &Graph,
    block: BlockId,
    kind: PointKind,
) -> Result<(ScopeId, u32, Resumes), VerifyError> {
    let here = graph.block(block);
    match kind {
        PointKind::Frame { node } => match &here.nodes[node] {
            Node::Frame {
                program_counter,
                instruction,
                ..
            } => Ok((
                here.scope,
                *program_counter,
                if instruction.argument_slots().is_some() {
                    Resumes::Deliver
                } else {
                    Resumes::Reexecute
                },
            )),
            _ => Err(VerifyError::Unaligned),
        },
        PointKind::Poll => {
            let head = graph.block(poll_target(graph, block).ok_or(VerifyError::Unaligned)?);
            let (mut scope, mut program_counter) = (head.scope, head.program_counter);
            while scope != here.scope {
                let (sender, call) = graph.scope(scope).parent.ok_or(VerifyError::Unaligned)?;
                scope = sender;
                program_counter = call;
            }
            Ok((scope, program_counter, Resumes::Reexecute))
        }
        PointKind::Trap { node: Some(node) } => here.nodes[node]
            .trap()
            .map(|program_counter| (here.scope, program_counter, Resumes::Reexecute))
            .ok_or(VerifyError::Unaligned),
        PointKind::Trap { node: None } => graph
            .end_trap(block)
            .map(|program_counter| (here.scope, program_counter, Resumes::Reexecute))
            .ok_or(VerifyError::Unaligned),
    }
}

/// Checks that `described` names the chain of scopes a frame stopped at
/// `stopped` stands in, innermost first, each delivering at the send its
/// inner scope is inlined at, and among each scope's locations every
/// register of its own that `tainted_here` leaves, in its frame word, and
/// no other.
fn described_chain(
    graph: &Graph,
    described: &InterruptPoint,
    stopped: (ScopeId, u32, Resumes),
    tainted_here: &[Range<u16>],
) -> Result<(), VerifyError> {
    let (_, program_counter, _) = stopped;
    let disagrees = VerifyError::MetadataDisagrees { program_counter };
    let mut expected = Some(stopped);
    let mut end = graph.scope(stopped.0).top();
    for scope in &described.scopes {
        let Some((id, counter, resumes)) = expected else {
            return Err(disagrees);
        };
        let owner = graph.scope(id);
        if scope.scope != id
            || scope.code_id != owner.code_id
            || scope.program_counter != counter
            || scope.resumes != resumes
            || scope.offset != owner.offset
        {
            return Err(disagrees);
        }
        let mut listed = scope.locations.iter();
        for register in owner.offset..end {
            if tainted(tainted_here, register) {
                continue;
            }
            if listed.next() != Some(&(register - owner.offset, Location::FrameWord)) {
                return Err(disagrees);
            }
        }
        if listed.next().is_some() {
            return Err(disagrees);
        }
        end = owner.offset;
        expected = owner
            .parent
            .map(|(sender, call)| (sender, call, Resumes::Deliver));
    }
    if expected.is_some() {
        return Err(disagrees);
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
    let tainted_there = graph.tainted_at_top(successor);
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
        if !assumed.clean
            && assumed.value.is_none()
            && !u16::try_from(register).is_ok_and(|number| tainted(&tainted_there, number))
        {
            return Err(disagrees);
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn replay_with_leaf_calls(
    graph: &Graph,
    allocation: &Allocation,
    metadata: &Metadata,
    leaf_calls: &[u32],
) -> Result<(), VerifyError> {
    replay(graph, allocation, metadata, leaf_calls)
}
