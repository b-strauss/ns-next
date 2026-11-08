//! The interrupt points' metadata (`docs/design/execution.md`, "Planned:
//! the deoptimization contract", "Optimized frames"): for every point at
//! which an observer may read optimized code's frame -- every frame node,
//! every back edge's poll, every trap -- the chain of scopes it stops
//! in, innermost first, and for each its program counter, which of the
//! two ways it resumes there, and where the value of each of its
//! registers the leftover analysis does not taint is. That is its frame
//! word: the point's write-back (`allocate::DirtyPoint`) stores every
//! binding that is not there yet, and `verify_allocation` holds the
//! allocation to it by replaying every move -- but for a register that
//! holds a block the compile eliminated on every way there
//! (`plan::ScopePlan::eliminated_before`), whose word holds `nil` for it:
//! there it is that block, which the exit makes (`jit::materialize`).

use std::collections::HashMap;
use std::ops::Range;

use super::allocate::{allocate, Allocation, Location, Move, PointKind, Spot};
use ns_assembler::Register;

use super::graph::{BlockId, Graph, GraphScope, Node, ScopeId, ValueId};
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
/// the header words the exit writes are not among them. A block scope's
/// activation's closure is no register: the exit writes it into the
/// header of the scope that runs the block (`jit::InlinedScope::block`).
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

impl InterruptPoint {
    /// Whether a register of a scope the point stops in holds a block the
    /// compile eliminated, which an exit there makes.
    pub(crate) fn holds_eliminated_blocks(&self) -> bool {
        self.scopes.iter().any(|scope| {
            scope
                .locations
                .iter()
                .any(|(_, location)| matches!(location, Location::EliminatedBlock(_)))
        })
    }
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

/// Where the value of `register` of `scope`, stopped before its
/// instruction at `program_counter`, is: the block the compile eliminated
/// that it holds on every way there, which its frame word holds `nil` for
/// until the exit makes it, or else its frame word -- always for one at or
/// past `callee_from`, where the callee of the send the frame stopped at
/// lies from the call on, its arguments copied in.
fn location_of(
    scope: &GraphScope,
    program_counter: u32,
    register: u16,
    callee_from: Option<u16>,
) -> Location {
    if callee_from.is_some_and(|start| register >= start) {
        return Location::FrameWord;
    }
    scope
        .eliminated_before
        .get(program_counter as usize)
        .and_then(|held| held.iter().find(|(holder, _)| *holder == register))
        .map_or(Location::FrameWord, |(_, block)| {
            Location::EliminatedBlock(*block)
        })
}

/// Where the callee of the send a frame node of `graph` stops at lies, in
/// its scope's numbering, for the interrupt point `kind` of `block`: from
/// the send's argument base up, the callee's frame, which a frame below
/// it reads none of its own blocks in; `None` for any other point -- a
/// trap or a type-case's, whose frame runs the send again, the arguments
/// it copies from those registers among what it reads.
fn callee_from(graph: &Graph, block: BlockId, kind: PointKind) -> Option<u16> {
    match kind {
        PointKind::Frame { node } => match &graph.block(block).nodes[node] {
            Node::Frame { instruction, .. } => instruction.argument_slots().map(|(base, _)| base),
            _ => None,
        },
        PointKind::Poll | PointKind::Trap { .. } => None,
    }
}

/// The chain of scopes a point that stops in `scope` at `program_counter`
/// and resumes as `resumes` describes (`ScopeAtPoint`): each register of
/// each scope the leftover analysis leaves to the frame there is in its
/// frame word, the point's write-back having stored every one that was
/// not, or the block the compile eliminated that it holds
/// (`location_of`), the innermost's past `callee_from` its callee's.
fn chain_at(
    graph: &Graph,
    (scope, program_counter, resumes): (ScopeId, u32, Resumes),
    callee_from: Option<u16>,
) -> Vec<ScopeAtPoint> {
    let tainted_here = graph.tainted_at(scope, program_counter);
    let mut chain = Vec::new();
    let mut stopped = (scope, program_counter, resumes);
    let mut end = graph.scope(scope).top();
    // Only the innermost scope's frame reaches past a send it stopped
    // at: every other ends where the one inlined into it begins.
    let mut callee_from = callee_from;
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
                .map(|register| {
                    let own = register - owner.offset;
                    (own, location_of(owner, program_counter, own, callee_from))
                })
                .collect(),
        });
        callee_from = None;
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
                let stopped = point_of(graph, point.block, point.kind);
                InterruptPoint {
                    scopes: chain_at(graph, stopped, callee_from(graph, point.block, point.kind)),
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
        // A register the leftover analysis taints at the block's top that
        // holds the interpreter's word holds what no one may read: a word
        // a callee or a popped frame left, which the collector's pre-pass
        // nils -- and, where a frame went back into the code at a poll,
        // the `nil` its entry restored a value no code reads on with
        // (`poll_restores`) -- so that a read of it before a write is
        // refused.
        let tainted_here = graph.tainted_at_top(*block);
        for (register, binding) in entry.bindings.iter().enumerate() {
            replay.interpreter[register] = binding.value;
            let tainted_word =
                u16::try_from(register).is_ok_and(|number| tainted(&tainted_here, number));
            replay.words[register] = match (binding.clean, binding.value) {
                (true, None) if tainted_word => Word::Stale,
                (true, None) => Word::Original,
                (true, Some(value)) => Word::Value(value),
                (false, _) => Word::Stale,
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
                    enters(&replay, graph, allocation, (*block, *fallback), None)?;
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
                | Node::ResetOwnSlotGuard
                | Node::Frame { .. }
                | Node::FieldLoad { .. }
                | Node::FieldStore { .. }
                | Node::IdentityCompare { .. }
                | Node::ClassOf { .. }
                | Node::IndexedLoad { .. }
                | Node::IndexedStore { .. }
                | Node::TrapIfNil { .. }
                | Node::EliminatedBlock { .. } => {}
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
            // A frame that goes back into the code at the poll.
            if let (Some(restores), Some(head)) = (
                poll_restores(graph, allocation, described, end_use),
                poll_target(graph, *block),
            ) {
                replay_reentry(
                    &stopped,
                    graph,
                    allocation,
                    described,
                    &restores,
                    (*block, head),
                    (
                        end_use,
                        positions.end_definition(*block),
                        &lives.live_in[head.index()],
                    ),
                )?;
            }
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
            enters(&replay, graph, allocation, (*block, successor), None)?;
        }
    }
    if points.next().is_some() {
        return Err(VerifyError::Unaligned);
    }
    Ok(())
}

/// Where a poll's entry reads a value back from (`poll_restores`): a frame
/// word the frame kept, a constant's word, or `nil` for a value no code
/// past the poll reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PollSource {
    Frame(u16),
    Word(u64),
    Nil,
}

/// One value a poll's entry restores: which, from where, into the machine
/// register the code holds it in, if any, and into each frame word the
/// collector's pre-pass may have nilled that the code reads it back from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PollRestore {
    pub(crate) value: ValueId,
    pub(crate) source: PollSource,
    pub(crate) machine: Option<Register>,
    pub(crate) lost: Vec<u16>,
}

/// The registers of the frame words the frame keeps at the interrupt
/// point `described`, in the physical frame's numbering: every register
/// of its chain the leftover analysis leaves to the frame, which the
/// collector traces and keeps current.
fn kept_at(described: &InterruptPoint) -> Vec<u16> {
    described
        .scopes
        .iter()
        .flat_map(|scope| {
            scope
                .locations
                .iter()
                .filter(|(_, location)| {
                    matches!(location, Location::FrameWord | Location::EliminatedBlock(_))
                })
                .map(move |(own, _)| scope.offset + own)
        })
        .collect()
}

/// What a frame going back into the code at the poll `described`, at
/// `position`, restores: every value live there in the machine register
/// the code holds it in, and in every frame word the code after the poll
/// reads it back from -- each from a frame word the frame kept, one of a
/// register the leftover analysis leaves to the frame there, which the
/// collector kept current meanwhile, or for a constant from its word,
/// since the collector's pre-pass nils every other (`Heap::trace_stack`).
/// A value the code past the poll never reads -- live only for the stores
/// of its dirty bindings, as a load of a slot the loop then stores is --
/// is read back from a kept word one of them names, which the poll's
/// write-back stored, so that the merge's stores write it again, or else
/// is `nil`, written only into words nothing reads, which the replay
/// holds the code to (`replay_reentry`). `None` where a value the code
/// reads on is in no such word and no constant: no frame goes back in
/// there, and the poll's exit is any interrupt point's
/// (`JitStatistics::polls_without_an_entry`).
pub(crate) fn poll_restores(
    graph: &Graph,
    allocation: &Allocation,
    described: &InterruptPoint,
    position: u32,
) -> Option<Vec<PollRestore>> {
    let kept = kept_at(described);
    let mut restores = Vec::new();
    for polled in allocation.poll_values.get(&position)? {
        // A value read on: every clean binding the code may read it back
        // from; one read by nothing but the stores of its dirty bindings:
        // none, its machine register alone.
        let lost: Vec<u16> = if polled.read_on {
            polled
                .clean
                .iter()
                .copied()
                .filter(|register| !kept.contains(register))
                .collect()
        } else {
            Vec::new()
        };
        if polled.machine.is_none() && lost.is_empty() {
            continue;
        }
        let source = match polled.bound.iter().find(|register| kept.contains(register)) {
            // A dirty binding the frame keeps, which the poll's write-back
            // stored, is a store the merge makes again.
            Some(register) => PollSource::Frame(*register),
            None => match graph.constant_word(polled.value) {
                Some(word) => PollSource::Word(word),
                None if !polled.read_on => PollSource::Nil,
                None => return None,
            },
        };
        restores.push(PollRestore {
            value: polled.value,
            source,
            machine: polled.machine,
            lost,
        });
    }
    Some(restores)
}

/// What no value is: a machine register a poll's entry gave `nil`
/// (`PollSource::Nil`), as the replay of the frame going back in knows it.
const RESTORED_NIL: ValueId = ValueId(u32::MAX);

/// Checks the frame going back into the code at a poll, whose state at
/// the poll after its write-back is `stopped`, `described` the point, as
/// `restores` restores it: the collector's pre-pass nils every frame word
/// the frame does not keep there, the entry writes back what `restores`
/// says, and the code past the poll -- the moves into the loop's head,
/// `head`, from `block` -- brings the head everything it assumes, as
/// `enters` checks every way in: a value no code reads on, `nil` in its
/// machine register, is stored only into words nothing reads, and every
/// clean binding the head assumes holds its value, but a tainted
/// register's binding of a value not in `live_at_head`, which no one
/// reads past the head. From the head on the
/// replay of every block holds the code to never reading a word the
/// analysis taints before it writes it, which a `nil` stored there is.
fn replay_reentry(
    stopped: &Replay,
    graph: &Graph,
    allocation: &Allocation,
    described: &InterruptPoint,
    restores: &[PollRestore],
    (block, head): (BlockId, BlockId),
    (end_use, end_definition, live_at_head): (u32, u32, &[ValueId]),
) -> Result<(), VerifyError> {
    let kept = kept_at(described);
    let mut reentered = stopped.clone();
    for (register, word) in reentered.words.iter_mut().enumerate() {
        if !u16::try_from(register).is_ok_and(|number| kept.contains(&number)) {
            *word = Word::Stale;
        }
    }
    reentered.machine.clear();
    for restore in restores {
        let held = match restore.source {
            PollSource::Frame(register) => {
                if stopped.words[usize::from(register)] != Word::Value(restore.value) {
                    return Err(VerifyError::StaleFrameWord {
                        register,
                        position: end_use,
                    });
                }
                restore.value
            }
            PollSource::Word(_) => restore.value,
            PollSource::Nil => RESTORED_NIL,
        };
        if let Some(register) = restore.machine {
            reentered.machine.insert(Spot::Machine(register), held);
        }
        for register in &restore.lost {
            reentered.words[usize::from(*register)] = Word::Value(held);
        }
    }
    // The moves into the head: a `nil` stored is a word nothing may read.
    for each in allocation.moves_at(end_definition) {
        match *each {
            Move::Store { from, register, .. }
                if reentered.machine.get(&Spot::Machine(from)) == Some(&RESTORED_NIL) =>
            {
                reentered.words[usize::from(register)] = Word::Stale;
            }
            Move::Transfer { from, into, .. }
                if reentered.machine.get(&from) == Some(&RESTORED_NIL) =>
            {
                reentered.machine.insert(into, RESTORED_NIL);
            }
            _ => apply_move(&mut reentered, allocation, *each, end_definition)?,
        }
    }
    enters(
        &reentered,
        graph,
        allocation,
        (block, head),
        Some(live_at_head),
    )
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
    for each in allocation.moves_at(position) {
        apply_move(replay, allocation, *each, position)?;
    }
    Ok(())
}

/// Replays the move `each` at `position`.
fn apply_move(
    replay: &mut Replay,
    allocation: &Allocation,
    each: Move,
    position: u32,
) -> Result<(), VerifyError> {
    let allocatable = |spot: Spot| match spot {
        Spot::Machine(register) => allocation.allocatable.contains(&register),
        Spot::Scratch => true,
    };
    match each {
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
        (&tainted_here, callee_from(graph, block, kind)),
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
    (tainted_here, mut callee_from): (&[Range<u16>], Option<u16>),
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
            let own = register - owner.offset;
            if listed.next() != Some(&(own, location_of(owner, counter, own, callee_from))) {
                return Err(disagrees);
            }
        }
        if listed.next().is_some() {
            return Err(disagrees);
        }
        callee_from = None;
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
/// it, unless the leftover analysis taints the register there -- and,
/// for a frame that went back in at a poll, `reentered_live` the values
/// live at the head, unless the value bound is no longer read.
fn enters(
    replay: &Replay,
    graph: &Graph,
    allocation: &Allocation,
    (predecessor, successor): (BlockId, BlockId),
    reentered_live: Option<&[ValueId]>,
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
        // The interpreter's word of a register tainted there is what no
        // one reads, whatever it holds.
        let tainted_word =
            u16::try_from(register).is_ok_and(|number| tainted(&tainted_there, number));
        // Past a poll's entry a tainted register's word may be the `nil`
        // the collector's pre-pass stored, whatever binding the head keeps
        // of it: no instruction reads it before writing it, and a value
        // no code past the head reads is read from it by no one either.
        let unread = tainted_word
            && assumed.value.is_none_or(|value| {
                reentered_live.is_some_and(|live| {
                    !live.contains(&value) && block.phis.iter().all(|phi| phi.value != value)
                })
            });
        if assumed.clean && !unread && !replay.in_sync(register) {
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
