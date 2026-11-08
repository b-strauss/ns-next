//! Register allocation over the optimizing compiler's graph
//! (`docs/design/execution.md`, "One tier, an SSA graph, the macro
//! assembler"): a linear scan in the order values are defined, over the
//! positions `liveness` numbers, the kind of Maglev's straight-forward
//! allocator -- each block entered with the state its way in leaves, a
//! merge's state decided where it is first reached and every way into it
//! given the moves that bring its own state there.
//!
//! **Where a value is.** In one of `Roles::allocatable`, or in its frame
//! word. A value lives only between two frame nodes (`graph`), so no
//! register's value outlives one; within that stretch, when no register
//! is free, the value whose next use in a register is farthest goes to
//! its frame word. A value has no frame slot of its own: it is a bytecode
//! register's value, and "in its frame word" means that every bytecode
//! register bound to it holds it in its frame word -- the spill stores it
//! into each that does not -- so a later use reloads it from any of them.
//! Every value but a constant or the receiver on its way to its `Copy` is
//! bound to some register while it lives, since a use reads a register's
//! binding and a binding is made only from another; those two are used at
//! the very next node and are never chosen. A `Copy` of a value in its
//! frame word reloads it, so that the new binding, whose frame word does
//! not hold it yet, never stands beside a value said to be in its frame
//! word.
//!
//! **Write-backs.** Before a frame node every binding whose frame word
//! does not hold its value yet is stored (`DirtyPoint`), so that the frame
//! is the interpreter's (decision 2); a back edge's poll records the same
//! stores for its cold path alone ("A poll writes nothing back",
//! `execution.md`, "Safepoints and the depth limit"), and so does a trap,
//! where a guard fails (`PointKind::Trap`); a way into a merge
//! stores each binding the merge does not carry on. A store left out would
//! leave a word the collector traces other than the interpreter's -- an
//! object kept alive that a `WeakArray` would show gone.
//!
//! Nothing here emits code, and no move takes a temporary but the idioms'
//! scratch register (`Spot::Scratch`), as `Roles::allocatable` requires.
use std::collections::BTreeMap;

use ns_assembler::Register;

use super::graph::{BlockId, Graph, Node, ValueId};
use super::liveness::{bind, poll_target, Binding, Intervals, Positions};
use super::plan::EliminatedId;
use crate::jit::macro_assembler::Roles;

/// Where a value is at a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Location {
    Register(Register),
    /// The frame word of every bytecode register bound to the value.
    FrameWord,
    /// A block the compile never creates, which an exit that finds it
    /// live materializes (`docs/design/execution.md`, "Inlining"): a
    /// register's location in an interrupt point's metadata, never a
    /// value's in the allocation, which holds the block's `nil` like any
    /// other value.
    EliminatedBlock(EliminatedId),
}

/// A place a move reads or writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Spot {
    Machine(Register),
    /// The idioms' scratch register (`Roles::idiom_scratch`), which breaks
    /// a cycle of moves.
    Scratch,
}

/// One move the code at a position makes, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Move {
    /// Stores `value`, held in `from`, into `register`'s frame word.
    Store {
        value: ValueId,
        from: Register,
        register: u16,
    },
    /// Loads `value` from `register`'s frame word into `into`.
    Load {
        value: ValueId,
        register: u16,
        into: Register,
    },
    /// Copies `value` from one place to another.
    Transfer {
        value: ValueId,
        from: Spot,
        into: Spot,
    },
}

/// The state a block is entered with, which every way into it brings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The values in machine registers.
    pub(crate) machine: Vec<(Register, ValueId)>,
    /// What each bytecode register is bound to, and whether its frame
    /// word holds it.
    pub(crate) bindings: Vec<Binding>,
}

/// Which interrupt point a `DirtyPoint` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointKind {
    /// The frame node at this index of the block's nodes.
    Frame { node: usize },
    /// The poll of the block's back edge.
    Poll,
    /// A trap: the failure of the integer fast path at this index of
    /// the block's nodes, or, for `None`, of the block's end
    /// (`Graph::end_trap`). Its write-back, like a poll's, lies on its
    /// way out alone.
    Trap { node: Option<usize> },
}

/// The write-back an interrupt point owes the frame: every binding whose
/// frame word does not hold its value there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirtyPoint {
    pub(crate) position: u32,
    pub(crate) block: BlockId,
    pub(crate) kind: PointKind,
    /// Each register to store, its value and where the value is.
    pub(crate) stores: Vec<(u16, ValueId, Location)>,
}

/// Where every value is at every position, and the moves and stores the
/// code makes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Allocation {
    pub(crate) positions: Positions,
    /// For each value, where it is from each of these positions on, in
    /// ascending order: its definition, each block it enters, each move.
    pub(crate) location_at: Vec<Vec<(u32, Location)>>,
    /// Each interrupt point's write-back, in the order of the positions.
    pub(crate) dirty_at: Vec<DirtyPoint>,
    /// The moves at each position: at a use position before the node or
    /// the end reads its operands, at a node's definition position after
    /// it, at an end's definition position on the way into the successor.
    pub(crate) moves: BTreeMap<u32, Vec<Move>>,
    /// Each block's entry state, `None` for a block not in the order.
    pub(crate) entries: Vec<Option<Entry>>,
    pub(crate) allocatable: &'static [Register],
    pub(crate) survive_leaf_calls: usize,
    /// Each back edge's poll, by its position: every value live there, as
    /// a frame going back into the code at the poll after its safepoint
    /// finds it again (`emit`'s poll entry).
    pub(crate) poll_values: BTreeMap<u32, Vec<PolledValue>>,
    ranges: Vec<Vec<(u32, u32)>>,
}

/// A value live at a back edge's poll: the machine register it is in, if
/// it is in one, every bytecode register bound to it -- each of whose
/// frame words holds it once the poll's write-back stored the dirty ones
/// -- and of those the ones whose frame words the code after the poll
/// takes to hold it, which it may read it back from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolledValue {
    pub(crate) value: ValueId,
    pub(crate) machine: Option<Register>,
    pub(crate) bound: Vec<u16>,
    pub(crate) clean: Vec<u16>,
}

impl Allocation {
    /// Where `value`, live at `position`, is there.
    pub(crate) fn location_of(&self, value: ValueId, position: u32) -> Location {
        let records = &self.location_at[value.index()];
        let before = records.partition_point(|(from, _)| *from <= position);
        assert!(
            before > 0,
            "internal: {value:?} is not placed at {position}"
        );
        records[before - 1].1
    }

    /// How many values live at `position` are in their frame words.
    #[cfg(test)]
    pub(crate) fn in_frame_word_at(&self, position: u32) -> usize {
        (0..self.location_at.len())
            .map(|index| ValueId(u32::try_from(index).expect("internal: a value id fits a u32")))
            .filter(|value| {
                let ranges = &self.ranges[value.index()];
                ranges
                    .iter()
                    .any(|(start, end)| *start <= position && position < *end)
                    && self.location_of(*value, position) == Location::FrameWord
            })
            .count()
    }

    /// The allocatable registers that hold a value live at `position`,
    /// where the code emitted for it runs: those a debug build holds
    /// against a temporary (`MacroAssembler::hold_values`).
    #[cfg(debug_assertions)]
    pub(crate) fn registers_held_at(&self, position: u32) -> Vec<Register> {
        self.ranges
            .iter()
            .enumerate()
            .filter(|(_, ranges)| {
                ranges
                    .iter()
                    .any(|(start, end)| *start <= position && position < *end)
            })
            .filter_map(|(index, _)| {
                let value = ValueId(u32::try_from(index).expect("internal: a value id fits a u32"));
                match self.location_of(value, position) {
                    Location::Register(register) => Some(register),
                    Location::FrameWord | Location::EliminatedBlock(_) => None,
                }
            })
            .collect()
    }

    /// The moves at `position`.
    pub(crate) fn moves_at(&self, position: u32) -> &[Move] {
        self.moves.get(&position).map_or(&[], Vec::as_slice)
    }
}

/// The allocation of `graph`'s values to `roles.allocatable` over
/// `intervals`. Every value is placed; none fails to be.
pub(crate) fn allocate(graph: &Graph, intervals: &Intervals, roles: &Roles) -> Allocation {
    let mut allocator = Allocator {
        graph,
        intervals,
        allocatable: roles.allocatable,
        survivors: roles.survive_leaf_calls,
        state: State::empty(roles.allocatable.len(), intervals.register_count),
        exits: BTreeMap::new(),
        allocation: Allocation {
            positions: intervals.positions.clone(),
            location_at: vec![Vec::new(); graph.values.len()],
            dirty_at: Vec::new(),
            moves: BTreeMap::new(),
            entries: vec![None; graph.blocks.len()],
            allocatable: roles.allocatable,
            survive_leaf_calls: roles.survive_leaf_calls,
            poll_values: BTreeMap::new(),
            ranges: intervals.ranges.clone(),
        },
    };
    for block in &intervals.positions.order {
        allocator.block(*block);
    }
    debug_assert!(
        allocator.exits.is_empty(),
        "internal: every way into a block was taken"
    );
    allocator.allocation
}

/// What the code holds at a position.
#[derive(Debug, Clone)]
struct State {
    /// The value in each allocatable register, by its index.
    machine: Vec<Option<ValueId>>,
    /// Where each placed value is.
    placed: BTreeMap<ValueId, Location>,
    bindings: Vec<Binding>,
}

impl State {
    fn empty(registers: usize, register_count: usize) -> State {
        State {
            machine: vec![None; registers],
            placed: BTreeMap::new(),
            bindings: vec![Binding::FRAME_WORD; register_count],
        }
    }

    /// Whether every bytecode register bound to `value` holds it in its
    /// frame word, and one is.
    fn backed(&self, value: ValueId) -> bool {
        let mut bound = self
            .bindings
            .iter()
            .filter(|binding| binding.value == Some(value))
            .peekable();
        bound.peek().is_some() && bound.all(|binding| binding.clean)
    }

    /// A bytecode register whose frame word holds `value`.
    fn frame_word_of(&self, value: ValueId) -> u16 {
        let register = self
            .bindings
            .iter()
            .position(|binding| {
                *binding
                    == Binding {
                        value: Some(value),
                        clean: true,
                    }
            })
            .expect("internal: a value in its frame word is a register's");
        u16::try_from(register).expect("internal: a register number fits a u16")
    }
}

struct Allocator<'graph> {
    graph: &'graph Graph,
    intervals: &'graph Intervals,
    allocatable: &'static [Register],
    survivors: usize,
    state: State,
    /// The state each way into a block not yet entered leaves with, by
    /// the way's block and the block entered.
    exits: BTreeMap<(BlockId, BlockId), State>,
    allocation: Allocation,
}

impl Allocator<'_> {
    fn positions(&self) -> &Positions {
        &self.intervals.positions
    }

    fn record(&mut self, value: ValueId, position: u32, location: Location) {
        self.allocation.location_at[value.index()].push((position, location));
    }

    fn push_moves(&mut self, position: u32, moves: Vec<Move>) {
        if !moves.is_empty() {
            self.allocation
                .moves
                .entry(position)
                .or_default()
                .extend(moves);
        }
    }

    fn block(&mut self, block: BlockId) {
        self.enter(block);
        let node_count = self.graph.block(block).nodes.len();
        for index in 0..node_count {
            self.node(block, index);
        }
        self.end(block);
    }

    fn enter(&mut self, block: BlockId) {
        let entry = self.positions().entry(block);
        let predecessors = &self.graph.block(block).predecessors;
        if block.index() == 0 {
            assert!(
                predecessors.is_empty(),
                "internal: the entry block has no way in"
            );
            self.state = State::empty(self.allocatable.len(), self.intervals.register_count);
        } else if predecessors.len() == 1 {
            let mut state = self
                .exits
                .remove(&(predecessors[0], block))
                .expect("internal: a block's one way in comes before it");
            let intervals = self.intervals;
            for slot in &mut state.machine {
                if slot.is_some_and(|value| !intervals.live_at(value, entry)) {
                    *slot = None;
                }
            }
            state
                .placed
                .retain(|value, _| intervals.live_at(*value, entry));
            self.state = state;
        } else {
            self.merge(block);
        }
        let placed: Vec<(ValueId, Location)> = self
            .state
            .placed
            .iter()
            .map(|(value, location)| (*value, *location))
            .collect();
        for (value, location) in placed {
            self.record(value, entry, location);
        }
        self.allocation.entries[block.index()] = Some(Entry {
            machine: self
                .state
                .machine
                .iter()
                .enumerate()
                .filter_map(|(index, value)| value.map(|value| (self.allocatable[index], value)))
                .collect(),
            bindings: self.state.bindings.clone(),
        });
    }

    /// Decides the state at the top of `merge`, reached first now, from
    /// the ways in already taken, and gives each of them its moves.
    fn merge(&mut self, merge: BlockId) {
        let graph = self.graph;
        let intervals = self.intervals;
        let entry = self.positions().entry(merge);
        let block = graph.block(merge);
        let taken: Vec<(usize, BlockId)> = block
            .predecessors
            .iter()
            .enumerate()
            .filter(|(_, predecessor)| self.exits.contains_key(&(**predecessor, merge)))
            .map(|(way, predecessor)| (way, *predecessor))
            .collect();
        let (first_way, first) = *taken
            .first()
            .expect("internal: a merge is reached after a way into it");
        let first_state = &self.exits[&(first, merge)];
        // Each value live at the top, with the value the first way in
        // brings for it.
        let mut live: Vec<(ValueId, ValueId)> = block
            .phis
            .iter()
            .map(|phi| (phi.value, phi.inputs[first_way]))
            .chain(
                intervals.live_in[merge.index()]
                    .iter()
                    .filter(|value| intervals.live_at(**value, entry))
                    .map(|value| (*value, *value)),
            )
            .collect();
        live.sort_by_key(|(value, _)| intervals.next_register_use(*value, entry));
        let capacity = self.allocatable.len();
        let mut machine: Vec<Option<ValueId>> = vec![None; capacity];
        let mut placed = BTreeMap::new();
        // The values used soonest take the registers, each where the first
        // way in has it if that one is free.
        let (registered, rest) = live.split_at(live.len().min(capacity));
        let mut unplaced = Vec::new();
        for (value, brought) in registered {
            let preferred = match first_state.placed.get(brought) {
                Some(Location::Register(register)) => self
                    .allocatable
                    .iter()
                    .position(|each| each == register)
                    .filter(|index| machine[*index].is_none()),
                _ => None,
            };
            match preferred {
                Some(index) => {
                    machine[index] = Some(*value);
                    placed.insert(*value, Location::Register(self.allocatable[index]));
                }
                None => unplaced.push(*value),
            }
        }
        for value in unplaced {
            let index = machine
                .iter()
                .position(Option::is_none)
                .expect("internal: as many registers as values to place");
            machine[index] = Some(value);
            placed.insert(value, Location::Register(self.allocatable[index]));
        }
        let mut bindings = intervals.entry_bindings[merge.index()]
            .clone()
            .expect("internal: a merge in the order has its bindings");
        for (value, _) in rest {
            placed.insert(*value, Location::FrameWord);
            let mut bound = false;
            for binding in &mut bindings {
                if binding.value == Some(*value) {
                    binding.clean = true;
                    bound = true;
                }
            }
            assert!(
                bound,
                "internal: a value live at a merge is a register's there"
            );
        }
        self.state = State {
            machine,
            placed,
            bindings,
        };
        // Recorded now, so that the ways in see it.
        self.allocation.entries[merge.index()] = Some(Entry {
            machine: self
                .state
                .machine
                .iter()
                .enumerate()
                .filter_map(|(index, value)| value.map(|value| (self.allocatable[index], value)))
                .collect(),
            bindings: self.state.bindings.clone(),
        });
        for (_, predecessor) in taken {
            assert!(
                graph.block(predecessor).successors().len() == 1,
                "internal: a way into a merge leaves a block with that way alone"
            );
            let exit = self
                .exits
                .remove(&(predecessor, merge))
                .expect("internal: a way taken");
            let moves = self.edge_moves(predecessor, merge, exit);
            let position = self.positions().end_definition(predecessor);
            self.push_moves(position, moves);
        }
    }

    /// The moves that bring `exit`, the state `predecessor` leaves with,
    /// to the state `merge` is entered with: each binding the merge
    /// assumes in its frame word stored, then the values into their
    /// registers, from registers in parallel and then from frame words.
    fn edge_moves(&self, predecessor: BlockId, merge: BlockId, mut exit: State) -> Vec<Move> {
        let block = self.graph.block(merge);
        let way = block
            .predecessors
            .iter()
            .position(|each| *each == predecessor)
            .expect("internal: a way into the merge");
        let brought = |value: ValueId| {
            block
                .phis
                .iter()
                .find(|phi| phi.value == value)
                .map_or(value, |phi| phi.inputs[way])
        };
        let entry = self.allocation.entries[merge.index()]
            .as_ref()
            .expect("internal: the merge's state is decided");
        let mut moves = Vec::new();
        for (register, assumed) in entry.bindings.iter().enumerate() {
            let held = &mut exit.bindings[register];
            if assumed.clean && !held.clean {
                let value = held
                    .value
                    .expect("internal: a binding not in its frame word has a value");
                let Some(Location::Register(from)) = exit.placed.get(&value) else {
                    panic!("internal: a value not in its frame word is in a register");
                };
                moves.push(Move::Store {
                    value,
                    from: *from,
                    register: u16::try_from(register)
                        .expect("internal: a register number fits a u16"),
                });
                held.clean = true;
            }
        }
        let mut transfers = Vec::new();
        let mut loads = Vec::new();
        for (into, value) in &entry.machine {
            let value = brought(*value);
            match exit.placed.get(&value) {
                Some(Location::Register(from)) => {
                    if from != into {
                        transfers.push((Spot::Machine(*from), Spot::Machine(*into), value));
                    }
                }
                Some(Location::FrameWord) => loads.push(Move::Load {
                    value,
                    register: exit.frame_word_of(value),
                    into: *into,
                }),
                Some(Location::EliminatedBlock(_)) | None => {
                    panic!("internal: a value live into a merge is placed on every way in")
                }
            }
        }
        moves.extend(sequence(transfers));
        moves.extend(loads);
        moves
    }

    /// Frees every register whose value is dead at `position`.
    fn free_dead(&mut self, position: u32) {
        let intervals = self.intervals;
        for slot in &mut self.state.machine {
            if slot.is_some_and(|value| !intervals.live_at(value, position)) {
                *slot = None;
            }
        }
        self.state
            .placed
            .retain(|value, _| intervals.live_at(*value, position));
    }

    /// A register for a value at `free_at`: one empty or whose value is
    /// dead there, or else the one whose value is next used in a register
    /// farthest from `position`, that value spilled there. `protected`
    /// keeps its values where they are.
    fn take_register(
        &mut self,
        free_at: u32,
        position: u32,
        protected: &[ValueId],
        moves: &mut Vec<Move>,
    ) -> usize {
        let intervals = self.intervals;
        if let Some(index) = self
            .state
            .machine
            .iter()
            .position(|slot| slot.is_none_or(|value| !intervals.live_at(value, free_at)))
        {
            return index;
        }
        let victim = (0..self.state.machine.len())
            .filter(|index| {
                let value = self.state.machine[*index].expect("internal: every register is taken");
                !protected.contains(&value)
                    && self
                        .state
                        .bindings
                        .iter()
                        .any(|binding| binding.value == Some(value))
            })
            .max_by_key(|index| {
                let value = self.state.machine[*index].expect("internal: every register is taken");
                intervals.next_register_use(value, position)
            })
            .expect("internal: a register holds a value that may go to its frame word");
        self.spill(victim, position, moves);
        victim
    }

    /// Sends the value in the register at `index` to its frame word,
    /// storing it into every register bound to it that does not hold it.
    fn spill(&mut self, index: usize, position: u32, moves: &mut Vec<Move>) {
        let value = self.state.machine[index]
            .take()
            .expect("internal: a spilled register holds a value");
        self.make_backed(value, self.allocatable[index], moves);
        self.state.placed.insert(value, Location::FrameWord);
        self.record(value, position, Location::FrameWord);
    }

    /// Stores `value`, held in `from`, into the frame word of every
    /// register bound to it that does not hold it.
    fn make_backed(&mut self, value: ValueId, from: Register, moves: &mut Vec<Move>) {
        for (register, binding) in self.state.bindings.iter_mut().enumerate() {
            if binding.value == Some(value) && !binding.clean {
                moves.push(Move::Store {
                    value,
                    from,
                    register: u16::try_from(register)
                        .expect("internal: a register number fits a u16"),
                });
                binding.clean = true;
            }
        }
    }

    /// Puts `value` in a register at `position`, reloading it from its
    /// frame word if it is there.
    fn ensure_register(
        &mut self,
        value: ValueId,
        position: u32,
        protected: &[ValueId],
        moves: &mut Vec<Move>,
    ) {
        match self.state.placed.get(&value) {
            Some(Location::Register(_)) => {}
            Some(Location::FrameWord) => {
                let index = self.take_register(position, position, protected, moves);
                if let Some(dead) = self.state.machine[index].take() {
                    self.state.placed.remove(&dead);
                }
                let into = self.allocatable[index];
                moves.push(Move::Load {
                    value,
                    register: self.state.frame_word_of(value),
                    into,
                });
                self.state.machine[index] = Some(value);
                self.state.placed.insert(value, Location::Register(into));
                self.record(value, position, Location::Register(into));
            }
            Some(Location::EliminatedBlock(_)) | None => {
                panic!("internal: {value:?} is used where it is not placed")
            }
        }
    }

    /// The write-back of every binding whose frame word does not hold its
    /// value, as the stores of an interrupt point.
    fn dirty_stores(&self) -> Vec<(u16, ValueId, Location)> {
        self.state
            .bindings
            .iter()
            .enumerate()
            .filter(|(_, binding)| !binding.clean && binding.value.is_some())
            .map(|(register, binding)| {
                let value = binding
                    .value
                    .expect("internal: a binding not in its frame word has a value");
                let location = self.state.placed[&value];
                assert!(
                    matches!(location, Location::Register(_)),
                    "internal: a value not in its frame word is in a register"
                );
                (
                    u16::try_from(register).expect("internal: a register number fits a u16"),
                    value,
                    location,
                )
            })
            .collect()
    }

    /// Every value live at a poll, here, where it is and the registers
    /// bound to it (`PolledValue`).
    fn polled_values(&self) -> Vec<PolledValue> {
        let number = |register: usize| {
            u16::try_from(register).expect("internal: a register number fits a u16")
        };
        self.state
            .placed
            .iter()
            .map(|(value, location)| {
                let bound = || {
                    self.state
                        .bindings
                        .iter()
                        .enumerate()
                        .filter(|(_, binding)| binding.value == Some(*value))
                };
                PolledValue {
                    value: *value,
                    machine: match location {
                        Location::Register(register) => Some(*register),
                        Location::FrameWord | Location::EliminatedBlock(_) => None,
                    },
                    bound: bound().map(|(register, _)| number(register)).collect(),
                    clean: bound()
                        .filter(|(_, binding)| binding.clean)
                        .map(|(register, _)| number(register))
                        .collect(),
                }
            })
            .collect()
    }

    fn node(&mut self, block: BlockId, index: usize) {
        let graph = self.graph;
        let intervals = self.intervals;
        let node = &graph.block(block).nodes[index];
        let use_position = self.positions().node_use(block, index);
        let definition = use_position + 1;
        self.free_dead(use_position);
        let mut moves = Vec::new();
        // A frame node's reads are its frame words, written back before it.
        let operands = match node {
            Node::Frame { .. } => Vec::new(),
            _ => node.uses(),
        };
        for operand in &operands {
            self.ensure_register(*operand, use_position, &operands, &mut moves);
        }
        if let Node::Frame { .. } = node {
            let stores = self.dirty_stores();
            self.allocation.dirty_at.push(DirtyPoint {
                position: use_position,
                block,
                kind: PointKind::Frame { node: index },
                stores,
            });
            self.push_moves(use_position, moves);
            debug_assert!(
                self.state
                    .placed
                    .keys()
                    .all(|value| !intervals.live_at(*value, definition)),
                "internal: no value lives across a frame node"
            );
            self.state.machine.fill(None);
            self.state.placed.clear();
            bind(&mut self.state.bindings, node);
            return;
        }
        let calls_leaf = intervals.leaf_calls.binary_search(&use_position).is_ok();
        if calls_leaf {
            self.keep_across_leaf_call(use_position, definition, &mut moves);
        }
        let defined = node.defined();
        // A frame read only a frame node reads, or no node, is never
        // loaded: the frame word it reads holds it.
        let target = defined.and_then(|value| {
            let loaded = !matches!(node, Node::FrameRead { .. })
                || !intervals.register_uses[value.index()].is_empty();
            loaded.then(|| self.take_register(definition, use_position, &operands, &mut moves))
        });
        if let Node::IntegerBinary {
            fallback: Some(fallback),
            ..
        } = node
        {
            if graph.block(block).exit_position(*fallback) == Some(index) {
                // Taken before the fast path writes its result, with every
                // move before it made.
                self.exits.insert((block, *fallback), self.state.clone());
            }
        }
        // A trap is taken where a fallback would be.
        if node.trap().is_some() {
            let stores = self.dirty_stores();
            self.allocation.dirty_at.push(DirtyPoint {
                position: use_position,
                block,
                kind: PointKind::Trap { node: Some(index) },
                stores,
            });
        }
        self.push_moves(use_position, moves);
        if calls_leaf {
            self.clobber_after_leaf_call(definition);
        }
        if let Some(value) = defined {
            let location = match target {
                Some(index) => {
                    if let Some(dead) = self.state.machine[index].replace(value) {
                        self.state.placed.remove(&dead);
                    }
                    Location::Register(self.allocatable[index])
                }
                None => Location::FrameWord,
            };
            self.state.placed.insert(value, location);
            self.record(value, definition, location);
        }
        bind(&mut self.state.bindings, node);
    }

    /// Before a simple node that calls a leaf: moves each value that
    /// lives past it out of a register the call clobbers into a free one
    /// it keeps, or else stores it into its frame words, where it is found
    /// after the call.
    fn keep_across_leaf_call(&mut self, use_position: u32, definition: u32, moves: &mut Vec<Move>) {
        let intervals = self.intervals;
        for index in self.survivors..self.state.machine.len() {
            let Some(value) = self.state.machine[index] else {
                continue;
            };
            if !intervals.live_at(value, definition) {
                continue;
            }
            let from = self.allocatable[index];
            match (0..self.survivors).find(|kept| self.state.machine[*kept].is_none()) {
                Some(kept) => {
                    let into = self.allocatable[kept];
                    moves.push(Move::Transfer {
                        value,
                        from: Spot::Machine(from),
                        into: Spot::Machine(into),
                    });
                    self.state.machine[index] = None;
                    self.state.machine[kept] = Some(value);
                    self.state.placed.insert(value, Location::Register(into));
                    self.record(value, use_position, Location::Register(into));
                }
                None => {
                    assert!(
                        self.state
                            .bindings
                            .iter()
                            .any(|binding| binding.value == Some(value)),
                        "internal: a value live across a leaf call is a register's"
                    );
                    self.make_backed(value, from, moves);
                }
            }
        }
    }

    /// After a leaf call: the registers it clobbers hold nothing, and
    /// what lives on of theirs is in its frame word.
    fn clobber_after_leaf_call(&mut self, definition: u32) {
        let intervals = self.intervals;
        for index in self.survivors..self.state.machine.len() {
            if let Some(value) = self.state.machine[index].take() {
                if intervals.live_at(value, definition) {
                    assert!(
                        self.state.backed(value),
                        "internal: a value live across a leaf call is in its frame word"
                    );
                    self.state.placed.insert(value, Location::FrameWord);
                    self.record(value, definition, Location::FrameWord);
                } else {
                    self.state.placed.remove(&value);
                }
            }
        }
    }

    fn end(&mut self, block: BlockId) {
        let graph = self.graph;
        let use_position = self.positions().end_use(block);
        self.free_dead(use_position);
        let mut moves = Vec::new();
        if let Some(value) = graph.block(block).end.uses() {
            self.ensure_register(value, use_position, &[value], &mut moves);
        }
        self.push_moves(use_position, moves);
        if poll_target(graph, block).is_some() {
            let stores = self.dirty_stores();
            self.allocation.dirty_at.push(DirtyPoint {
                position: use_position,
                block,
                kind: PointKind::Poll,
                stores,
            });
            let values = self.polled_values();
            self.allocation.poll_values.insert(use_position, values);
        }
        if graph.end_trap(block).is_some() {
            let stores = self.dirty_stores();
            self.allocation.dirty_at.push(DirtyPoint {
                position: use_position,
                block,
                kind: PointKind::Trap { node: None },
                stores,
            });
        }
        for successor in graph.block(block).end.successors() {
            let merges = graph.block(successor).predecessors.len() > 1;
            if merges && self.allocation.entries[successor.index()].is_some() {
                assert!(
                    graph.block(block).successors().len() == 1,
                    "internal: a way into a merge leaves a block with that way alone"
                );
                let moves = self.edge_moves(block, successor, self.state.clone());
                let position = self.positions().end_definition(block);
                self.push_moves(position, moves);
            } else {
                self.exits.insert((block, successor), self.state.clone());
            }
        }
    }
}

/// The moves of `transfers`, made at once, in an order that reads every
/// source before it is overwritten, a cycle broken through the scratch
/// register. No two transfers write one place.
fn sequence(mut transfers: Vec<(Spot, Spot, ValueId)>) -> Vec<Move> {
    let mut moves = Vec::with_capacity(transfers.len() + 1);
    while !transfers.is_empty() {
        let ready = transfers.iter().position(|(_, into, _)| {
            transfers
                .iter()
                .all(|(from, other_into, _)| from != into || other_into == into)
        });
        match ready {
            Some(index) => {
                let (from, into, value) = transfers.remove(index);
                moves.push(Move::Transfer { value, from, into });
            }
            None => {
                let (from, _, value) = transfers[0];
                moves.push(Move::Transfer {
                    value,
                    from,
                    into: Spot::Scratch,
                });
                for transfer in &mut transfers {
                    if transfer.0 == from {
                        transfer.0 = Spot::Scratch;
                    }
                }
            }
        }
    }
    moves
}

#[cfg(test)]
mod tests;
