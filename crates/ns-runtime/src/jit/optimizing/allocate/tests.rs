use super::{allocate, sequence, Location, Move, Spot};
use crate::bytecode::{Instruction, IntegerOperation, Literal};
use crate::jit::macro_assembler::{Abi, Roles};
use crate::jit::optimizing::build::build;
use crate::jit::optimizing::graph::{BlockId, Graph, Node};
use crate::jit::optimizing::liveness::{block_order, intervals};
use crate::jit::optimizing::metadata::{
    allocate_and_describe, metadata, replay_with_leaf_calls, verify_allocation, Resumes,
};
use crate::jit::optimizing::snapshot::Snapshot;
use crate::jit::optimizing::verify::VerifyError;
use crate::jit::optimizing::{corpus_of_bodies, shipped_bodies, test_send, test_snapshot};

fn allocated(snapshot: &Snapshot, abi: Abi) -> (Graph, super::Allocation) {
    let graph = build(snapshot);
    let allocation = allocate(
        &graph,
        &intervals(&graph, &block_order(&graph)),
        &Roles::of(abi),
    );
    (graph, allocation)
}

fn verified(
    graph: &Graph,
    allocation: &super::Allocation,
    snapshot: &Snapshot,
) -> Result<(), VerifyError> {
    verify_allocation(
        graph,
        allocation,
        &metadata(graph, allocation, snapshot),
        snapshot,
    )
}

/// `r3 := self. r5 := 1. r6 := r3 new: r5. ^r3`
fn a_send_after_two_bindings() -> Snapshot {
    test_snapshot(vec![
        Instruction::LoadSelf { destination: 3 },
        Instruction::LoadLiteral {
            destination: 5,
            literal: Literal::SmallInteger(1),
        },
        test_send(3, 5, 6),
        Instruction::Return { register: 3 },
    ])
}

#[test]
fn a_value_used_across_a_send_is_in_its_frame_word_at_the_send() {
    let snapshot = a_send_after_two_bindings();
    let (graph, allocation) = allocated(&snapshot, Abi::Windows64);
    let metadata = metadata(&graph, &allocation, &snapshot);
    let send = metadata
        .points
        .iter()
        .find(|point| point.program_counter == 2)
        .expect("the send is an interrupt point");
    assert_eq!(send.resumes, Resumes::Deliver);
    assert!(send.locations.iter().any(|(register, _)| *register == 3));
    assert!(send
        .locations
        .iter()
        .all(|(_, location)| *location == Location::FrameWord));
    // Both bindings are stored before the send, from their registers.
    let stored: Vec<u16> = allocation.dirty_at[0]
        .stores
        .iter()
        .map(|(register, _, _)| *register)
        .collect();
    assert_eq!(stored, vec![3, 5]);
    assert_eq!(verified(&graph, &allocation, &snapshot), Ok(()));
}

#[test]
fn a_send_without_its_write_back_is_refused() {
    let snapshot = a_send_after_two_bindings();
    let (graph, mut allocation) = allocated(&snapshot, Abi::SystemV);
    allocation.dirty_at[0].stores.remove(0);
    assert_eq!(
        verified(&graph, &allocation, &snapshot),
        Err(VerifyError::NotWrittenBack {
            register: 3,
            program_counter: 2
        })
    );
}

/// `r3 := 0. [r3 := r3 + 1. r3 < 10] whileTrue. ^r3`, each operation's
/// fast path beside its fallback send: the loop's fast path holds no
/// send.
fn counting_loop() -> Snapshot {
    test_snapshot(vec![
        Instruction::LoadLiteral {
            destination: 3,
            literal: Literal::SmallInteger(0),
        },
        Instruction::LoadLiteral {
            destination: 5,
            literal: Literal::SmallInteger(1),
        },
        Instruction::IntegerBinary {
            operation: IntegerOperation::Add,
            destination: 6,
            left: 3,
            right: 5,
            target: 4,
        },
        Instruction::Jump { target: 5 },
        test_send(3, 5, 6),
        Instruction::Move {
            destination: 3,
            source: 6,
        },
        Instruction::LoadLiteral {
            destination: 7,
            literal: Literal::SmallInteger(10),
        },
        Instruction::IntegerBinary {
            operation: IntegerOperation::Less,
            destination: 4,
            left: 3,
            right: 7,
            target: 9,
        },
        Instruction::Jump { target: 10 },
        test_send(3, 7, 4),
        Instruction::BranchIfTrue {
            register: 4,
            target: 1,
        },
        Instruction::Return { register: 3 },
    ])
}

#[test]
fn a_loop_counter_stays_in_a_register_across_the_loop() {
    let snapshot = counting_loop();
    let (graph, allocation) = allocated(&snapshot, Abi::Windows64);
    let head = graph.loop_heads().next().expect("the body loops");
    let counter = &graph.block(head).phis[0];
    assert_eq!(counter.register, 3);
    let positions = &allocation.positions;
    assert!(matches!(
        allocation.location_of(counter.value, positions.entry(head)),
        Location::Register(_)
    ));
    // The way back brings the next count in a register too, and stores it
    // only on its poll's cold path.
    let (way, back) = graph
        .block(head)
        .predecessors
        .iter()
        .enumerate()
        .find(|(_, predecessor)| **predecessor >= head)
        .expect("a way back");
    let loop_end_position = positions.end_use(*back);
    assert!(matches!(
        allocation.location_of(counter.inputs[way], loop_end_position),
        Location::Register(_)
    ));
    assert!(allocation
        .moves_at(loop_end_position)
        .iter()
        .chain(allocation.moves_at(loop_end_position + 1))
        .all(|each| !matches!(each, Move::Store { register: 3, .. })));
    let poll = allocation
        .dirty_at
        .iter()
        .find(|point| point.position == loop_end_position)
        .expect("the way back polls");
    assert!(poll.stores.iter().any(|(register, _, _)| *register == 3));
    assert_eq!(verified(&graph, &allocation, &snapshot), Ok(()));
}

/// Ten frame reads, each copied into a register of its own, then each
/// copied back: ten values live at once.
fn ten_live_values() -> Snapshot {
    let mut instructions = Vec::new();
    for index in 0..10 {
        instructions.push(Instruction::Move {
            destination: 11 + index,
            source: 1 + index,
        });
    }
    for index in 0..10 {
        instructions.push(Instruction::Move {
            destination: 1 + index,
            source: 11 + index,
        });
    }
    instructions.push(Instruction::Return { register: 1 });
    test_snapshot(instructions)
}

#[test]
fn more_live_values_than_registers_spill_the_farthest_used() {
    let snapshot = ten_live_values();
    let (graph, allocation) = allocated(&snapshot, Abi::Windows64);
    assert_eq!(Roles::of(Abi::Windows64).allocatable.len(), 8);
    // The tenth frame read: the first ten reads and copies are nodes 0 to
    // 19.
    let Node::FrameRead { register: 10, .. } = graph.blocks[0].nodes[18] else {
        panic!("the tenth read");
    };
    let peak_position = allocation.positions.node_definition(BlockId(0), 18);
    assert_eq!(allocation.in_frame_word_at(peak_position), 2);
    // The two next used last of those then live: the eighth, then the
    // ninth, whose own copy is behind it by then.
    for node in [14, 16] {
        let Node::FrameRead { value, .. } = graph.blocks[0].nodes[node] else {
            panic!("a read");
        };
        assert_eq!(
            allocation.location_of(value, peak_position),
            Location::FrameWord
        );
    }
    assert_eq!(verified(&graph, &allocation, &snapshot), Ok(()));
}

#[test]
fn a_use_whose_reload_is_left_out_is_refused() {
    let snapshot = ten_live_values();
    let (graph, mut allocation) = allocated(&snapshot, Abi::Windows64);
    for moves in allocation.moves.values_mut() {
        moves.retain(|each| !matches!(each, Move::Load { .. }));
    }
    assert!(matches!(
        verified(&graph, &allocation, &snapshot),
        Err(VerifyError::Misplaced { .. })
    ));
}

#[test]
fn a_value_live_across_a_leaf_call_leaves_the_registers_it_clobbers() {
    let snapshot = ten_live_values();
    let graph = build(&snapshot);
    let order = block_order(&graph);
    let plain = intervals(&graph, &order);
    // Pretend the eighth copy calls a leaf: seven values live past it.
    let leaf = plain.positions.node_use(BlockId(0), 15);
    let mut with_leaf = plain.clone();
    with_leaf.leaf_calls.push(leaf);
    let roles = Roles::of(Abi::SystemV);
    let kept = allocate(&graph, &with_leaf, &roles);
    let described = metadata(&graph, &kept, &snapshot);
    assert_eq!(
        replay_with_leaf_calls(&graph, &kept, &described, &snapshot, &[leaf]),
        Ok(())
    );
    let unkept = allocate(&graph, &plain, &roles);
    let described = metadata(&graph, &unkept, &snapshot);
    assert!(matches!(
        replay_with_leaf_calls(&graph, &unkept, &described, &snapshot, &[leaf]),
        Err(VerifyError::LeafClobbered { .. })
    ));
}

/// `r3 := self. r3 ifTrue: [r7 := 5] ifFalse: [r7 := r3]. r6 := r3 new:
/// r3. ^r6` -- r7 is dead where the arms meet, but no arm's binding of it
/// may be left unstored before the send.
fn a_register_dead_where_two_arms_meet() -> Snapshot {
    test_snapshot(vec![
        Instruction::LoadSelf { destination: 3 },
        Instruction::BranchIfTrue {
            register: 3,
            target: 4,
        },
        Instruction::Move {
            destination: 7,
            source: 3,
        },
        Instruction::Jump { target: 5 },
        Instruction::LoadLiteral {
            destination: 7,
            literal: Literal::SmallInteger(5),
        },
        test_send(3, 3, 6),
        Instruction::Return { register: 6 },
    ])
}

#[test]
fn a_binding_a_merge_does_not_carry_is_stored_on_its_way_in() {
    let snapshot = a_register_dead_where_two_arms_meet();
    let (graph, allocation) = allocated(&snapshot, Abi::Aapcs64);
    let merge = graph
        .blocks
        .iter()
        .position(|block| block.predecessors.len() == 2)
        .map(BlockId::of_index)
        .expect("the arms meet");
    for predecessor in &graph.block(merge).predecessors {
        let position = allocation.positions.end_definition(*predecessor);
        assert!(
            allocation
                .moves_at(position)
                .iter()
                .any(|each| matches!(each, Move::Store { register: 7, .. })),
            "each arm stores r7 on its way in"
        );
    }
    assert_eq!(verified(&graph, &allocation, &snapshot), Ok(()));

    let mut unstored = allocation.clone();
    let position = unstored
        .positions
        .end_definition(graph.block(merge).predecessors[0]);
    unstored
        .moves
        .get_mut(&position)
        .expect("the arm's moves")
        .retain(|each| !matches!(each, Move::Store { register: 7, .. }));
    assert!(matches!(
        verified(&graph, &unstored, &snapshot),
        Err(VerifyError::EdgeDisagrees { .. })
    ));
}

/// `r13 := self. r13 ifTrue: [r14 := 5] ifFalse: [r20 := r13]. r16 :=
/// r13 new: r13. ^r16` -- r20 is tainted where the arms meet: the way
/// that does not write it brings it tainted from the body's entry.
fn a_tainted_register_where_two_arms_meet() -> Snapshot {
    test_snapshot(vec![
        Instruction::LoadSelf { destination: 13 },
        Instruction::BranchIfTrue {
            register: 13,
            target: 4,
        },
        Instruction::Move {
            destination: 20,
            source: 13,
        },
        Instruction::Jump { target: 5 },
        Instruction::LoadLiteral {
            destination: 14,
            literal: Literal::SmallInteger(5),
        },
        test_send(13, 13, 16),
        Instruction::Return { register: 16 },
    ])
}

#[test]
fn a_binding_the_leftover_analysis_taints_at_a_merge_is_not_stored() {
    let snapshot = a_tainted_register_where_two_arms_meet();
    assert!(snapshot.tainted_before[5]
        .iter()
        .any(|range| range.contains(&20)));
    let graph = build(&snapshot);
    let stores_of_r20 = |allocation: &super::Allocation| {
        allocation
            .moves
            .values()
            .flatten()
            .filter(|each| matches!(each, Move::Store { register: 20, .. }))
            .count()
    };
    let (knowing, described) = allocate_and_describe(&snapshot, &graph, &Roles::of(Abi::Windows64));
    assert_eq!(stores_of_r20(&knowing), 0);
    assert_eq!(
        verify_allocation(&graph, &knowing, &described, &snapshot),
        Ok(())
    );
    let (_, unknowing) = allocated(&snapshot, Abi::Windows64);
    assert_eq!(stores_of_r20(&unknowing), 1);
    assert_eq!(verified(&graph, &unknowing, &snapshot), Ok(()));
}

#[test]
fn a_cycle_of_moves_goes_through_the_scratch_register() {
    use ns_assembler::Register;
    use std::collections::HashMap;

    let first = Spot::Machine(Register(1));
    let second = Spot::Machine(Register(2));
    let third = Spot::Machine(Register(3));
    let values = [
        crate::jit::optimizing::graph::ValueId(0),
        crate::jit::optimizing::graph::ValueId(1),
        crate::jit::optimizing::graph::ValueId(2),
    ];
    let moves = sequence(vec![
        (first, second, values[0]),
        (second, third, values[1]),
        (third, first, values[2]),
        (first, Spot::Machine(Register(4)), values[0]),
    ]);
    let mut held: HashMap<Spot, _> =
        HashMap::from([(first, values[0]), (second, values[1]), (third, values[2])]);
    for each in &moves {
        let Move::Transfer { value, from, into } = *each else {
            panic!("only transfers");
        };
        assert_eq!(
            held[&from], value,
            "a source is read before it is overwritten"
        );
        held.insert(into, value);
    }
    assert_eq!(held[&second], values[0]);
    assert_eq!(held[&third], values[1]);
    assert_eq!(held[&first], values[2]);
    assert_eq!(held[&Spot::Machine(Register(4))], values[0]);
    assert_eq!(
        moves.len(),
        5,
        "one move more than the transfers, through the scratch"
    );
}

#[test]
fn every_built_graph_allocates_and_verifies_on_both_abis() {
    let mut spilled = false;
    let mut points = 0;
    for snapshot in corpus_of_bodies().iter().chain(&shipped_bodies()) {
        for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
            let graph = build(snapshot);
            let (allocation, described) = allocate_and_describe(snapshot, &graph, &Roles::of(abi));
            assert_eq!(
                verify_allocation(&graph, &allocation, &described, snapshot),
                Ok(())
            );
            // Knowing nothing of the leftover analysis stores more, and
            // holds as well.
            let (graph, unknowing) = allocated(snapshot, abi);
            assert_eq!(verified(&graph, &unknowing, snapshot), Ok(()));
            spilled |= allocation
                .location_at
                .iter()
                .flatten()
                .any(|(_, location)| *location == Location::FrameWord);
            points += allocation.dirty_at.len();
        }
    }
    assert!(spilled, "some value is in its frame word somewhere");
    assert!(points > 0);
}

/// `r5 := 1. r6 := r3 new: r5. [r3 := r3 new: r5. r3 < r5] whileTrue.
/// ^r3`: every way into the loop's head, and into the merge of the
/// comparison's fast path and its fallback, brings r3 as its frame word,
/// so r3 is never reloaded on a way in nor stored back -- only the
/// literal and the comparison's result, which the branch reads, are.
fn a_loop_that_writes_a_register_only_by_a_send() -> Snapshot {
    test_snapshot(vec![
        Instruction::LoadLiteral {
            destination: 5,
            literal: Literal::SmallInteger(1),
        },
        test_send(3, 5, 6),
        test_send(3, 5, 3),
        Instruction::IntegerBinary {
            operation: IntegerOperation::Less,
            destination: 4,
            left: 3,
            right: 5,
            target: 6,
        },
        Instruction::BranchIfTrue {
            register: 4,
            target: 2,
        },
        Instruction::Return { register: 3 },
        test_send(3, 5, 4),
        Instruction::Jump { target: 4 },
    ])
}

#[test]
fn a_register_every_way_brings_as_its_frame_word_is_neither_reloaded_nor_stored() {
    let snapshot = a_loop_that_writes_a_register_only_by_a_send();
    for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
        let (graph, allocation) = allocated(&snapshot, abi);
        assert_eq!(verified(&graph, &allocation, &snapshot), Ok(()));
        let moves = || allocation.moves.values().flatten();
        assert!(
            moves().all(|each| !matches!(each, Move::Load { register: 3, .. })),
            "no way in reloads r3"
        );
        let stored: Vec<u16> = allocation
            .dirty_at
            .iter()
            .flat_map(|point| point.stores.iter().map(|(register, _, _)| *register))
            .chain(moves().filter_map(|each| match each {
                Move::Store { register, .. } => Some(*register),
                _ => None,
            }))
            .collect();
        assert!(stored.contains(&5), "the literal is written back");
        assert!(!stored.contains(&3), "r3 is never stored: {stored:?}");
    }
}

#[test]
fn the_frame_word_rule_leaves_a_write_back_left_out_refused() {
    let snapshot = a_loop_that_writes_a_register_only_by_a_send();
    let (graph, mut allocation) = allocated(&snapshot, Abi::SystemV);
    let first = allocation
        .dirty_at
        .iter_mut()
        .find(|point| !point.stores.is_empty())
        .expect("the literal's write-back");
    first.stores.retain(|(register, _, _)| *register != 5);
    assert_eq!(
        verified(&graph, &allocation, &snapshot),
        Err(VerifyError::NotWrittenBack {
            register: 5,
            program_counter: 1
        })
    );
}
