use super::{allocate, sequence, Location, Move, PointKind, Spot};
use crate::bytecode::{Instruction, IntegerOperation, Literal};
use crate::jit::macro_assembler::{Abi, Roles};
use crate::jit::optimizing::build::build_speculating as build;
use crate::jit::optimizing::graph::{BlockId, Graph, Node};
use crate::jit::optimizing::liveness::{block_order, intervals};
use crate::jit::optimizing::metadata::{
    allocate_and_describe, metadata, replay_with_leaf_calls, verify_allocation, Resumes,
};
use crate::jit::optimizing::snapshot::Snapshot;
use crate::jit::optimizing::verify::VerifyError;
use crate::jit::optimizing::{
    corpus_of_bodies, shipped_bodies, test_send, test_snapshot, unrun_every_site, unrun_site,
};

fn allocated(snapshot: &Snapshot, abi: Abi) -> (Graph, super::Allocation) {
    let graph = build(snapshot);
    let allocation = allocate(
        &graph,
        &intervals(&graph, &block_order(&graph)),
        &Roles::of_abi(abi),
    );
    (graph, allocation)
}

fn verified(graph: &Graph, allocation: &super::Allocation) -> Result<(), VerifyError> {
    verify_allocation(graph, allocation, &metadata(graph, allocation))
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
    let metadata = metadata(&graph, &allocation);
    let send = metadata
        .points
        .iter()
        .find(|point| point.scopes[0].program_counter == 2)
        .expect("the send is an interrupt point");
    assert_eq!(send.scopes[0].resumes, Resumes::Deliver);
    assert!(send.scopes[0]
        .locations
        .iter()
        .any(|(register, _)| *register == 3));
    assert!(send.scopes[0]
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
    assert_eq!(verified(&graph, &allocation), Ok(()));
}

#[test]
fn a_send_without_its_write_back_is_refused() {
    let snapshot = a_send_after_two_bindings();
    let (graph, mut allocation) = allocated(&snapshot, Abi::SystemV);
    allocation.dirty_at[0].stores.remove(0);
    assert_eq!(
        verified(&graph, &allocation),
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
    assert_eq!(verified(&graph, &allocation), Ok(()));
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
    assert_eq!(Roles::of_abi(Abi::Windows64).allocatable.len(), 8);
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
    assert_eq!(verified(&graph, &allocation), Ok(()));
}

#[test]
fn a_use_whose_reload_is_left_out_is_refused() {
    let snapshot = ten_live_values();
    let (graph, mut allocation) = allocated(&snapshot, Abi::Windows64);
    for moves in allocation.moves.values_mut() {
        moves.retain(|each| !matches!(each, Move::Load { .. }));
    }
    assert!(matches!(
        verified(&graph, &allocation),
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
    let roles = Roles::of_abi(Abi::SystemV);
    let kept = allocate(&graph, &with_leaf, &roles);
    let described = metadata(&graph, &kept);
    assert_eq!(
        replay_with_leaf_calls(&graph, &kept, &described, &[leaf]),
        Ok(())
    );
    let unkept = allocate(&graph, &plain, &roles);
    let described = metadata(&graph, &unkept);
    assert!(matches!(
        replay_with_leaf_calls(&graph, &unkept, &described, &[leaf]),
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
    assert_eq!(verified(&graph, &allocation), Ok(()));

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
        verified(&graph, &unstored),
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
    let (knowing, described) = allocate_and_describe(&graph, &Roles::of_abi(Abi::Windows64));
    assert_eq!(stores_of_r20(&knowing), 0);
    assert_eq!(verify_allocation(&graph, &knowing, &described), Ok(()));
    let (_, unknowing) = allocated(&snapshot, Abi::Windows64);
    assert_eq!(stores_of_r20(&unknowing), 1);
    assert_eq!(verified(&graph, &unknowing), Ok(()));
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
            let (allocation, described) = allocate_and_describe(&graph, &Roles::of_abi(abi));
            assert_eq!(verify_allocation(&graph, &allocation, &described), Ok(()));
            // Knowing nothing of the leftover analysis stores more, and
            // holds as well.
            let (graph, unknowing) = allocated(snapshot, abi);
            assert_eq!(verified(&graph, &unknowing), Ok(()));
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

/// Every body that reads an own slot, built as a customized copy, its
/// reads simple nodes, allocates and verifies on both ABIs.
#[test]
fn every_customized_graph_allocates_and_verifies_on_both_abis() {
    let id = crate::mixin::DispatchId::from_bits(7);
    let customization = crate::jit::optimizing::snapshot::Customization {
        key: crate::jit::CustomizationKey {
            receiver: id,
            entered: id,
        },
        own_slot_base: 3,
    };
    let mut loads = 0;
    for mut snapshot in corpus_of_bodies().into_iter().chain(shipped_bodies()) {
        snapshot.customization = Some(customization);
        let graph = build(&snapshot);
        let reads = graph
            .blocks
            .iter()
            .flat_map(|block| &block.nodes)
            .filter(|node| matches!(node, Node::OwnSlotLoad { .. }))
            .count();
        if reads == 0 {
            continue;
        }
        loads += reads;
        for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
            let (allocation, described) = allocate_and_describe(&graph, &Roles::of_abi(abi));
            assert_eq!(verify_allocation(&graph, &allocation, &described), Ok(()));
        }
    }
    assert!(loads > 0, "the library's lazy getters read their slots");
}

/// `r5 := 1. r6 := r3 new: r5. [r3 := r3 new: r5. r3 < r5] whileTrue.
/// ^r3`: every way into the loop's head, and into the merge of the
/// comparison's fast path and its fallback, brings r3 as its frame word,
/// so r3 is never reloaded on a way in nor stored back -- only the literal
/// and the comparison's result, which the branch reads, are.
fn a_loop_that_writes_a_register_only_by_a_send() -> Snapshot {
    test_snapshot(vec![
        Instruction::LoadLiteral {
            destination: 5,
            literal: Literal::SmallInteger(1),
        },
        test_send(3, 5, 6),
        // 2: the loop's head.
        test_send(3, 5, 3),
        Instruction::IntegerBinary {
            operation: IntegerOperation::Less,
            destination: 4,
            left: 3,
            right: 5,
            target: 5,
        },
        Instruction::Jump { target: 6 },
        test_send(3, 5, 4),
        Instruction::BranchIfTrue {
            register: 4,
            target: 2,
        },
        Instruction::Return { register: 3 },
    ])
}

#[test]
fn a_register_every_way_brings_as_its_frame_word_is_neither_reloaded_nor_stored() {
    let snapshot = a_loop_that_writes_a_register_only_by_a_send();
    for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
        let (graph, allocation) = allocated(&snapshot, abi);
        assert_eq!(verified(&graph, &allocation), Ok(()));
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
        verified(&graph, &allocation),
        Err(VerifyError::NotWrittenBack {
            register: 5,
            program_counter: 1
        })
    );
}

/// `t := r1. r1 := 0. <nine literals>. r12 := r1. r1 := t. <branch>.
/// ^r1`: under the pressure of the literals the `0` is spilled into r1's
/// frame word, and r1 is then bound again to the read of its frame word
/// made before; where the branch's ways meet, both bring that read, which
/// no longer is what the frame word holds.
fn a_register_bound_again_to_its_earlier_read() -> Snapshot {
    let mut instructions = vec![
        Instruction::Move {
            destination: 20,
            source: 1,
        },
        Instruction::LoadLiteral {
            destination: 1,
            literal: Literal::SmallInteger(0),
        },
    ];
    for index in 0..9 {
        instructions.push(Instruction::LoadLiteral {
            destination: 2 + index,
            literal: Literal::SmallInteger(i64::from(index) + 10),
        });
    }
    for index in 0..9 {
        instructions.push(Instruction::Move {
            destination: 21 + index,
            source: 2 + index,
        });
    }
    instructions.extend([
        Instruction::Move {
            destination: 12,
            source: 1,
        },
        Instruction::Move {
            destination: 1,
            source: 20,
        },
        Instruction::BranchIfTrue {
            register: 12,
            target: 24,
        },
        Instruction::Move {
            destination: 13,
            source: 12,
        },
        Instruction::Return { register: 1 },
    ]);
    test_snapshot(instructions)
}

#[test]
fn a_register_bound_again_to_its_earlier_read_is_written_back() {
    let snapshot = a_register_bound_again_to_its_earlier_read();
    for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
        let (graph, allocation) = allocated(&snapshot, abi);
        assert_eq!(verified(&graph, &allocation), Ok(()), "{abi:?}");
    }
}

/// `r13 := 1. r14 := self. r15 := r13 + r13. ^r15`, the sum's fallback
/// never run: the trap owes the frame both bindings the code holds dirty
/// there, on its cold path alone.
#[test]
fn a_trap_writes_back_every_dirty_binding() {
    for mut snapshot in corpus_of_bodies().into_iter().chain(shipped_bodies()) {
        unrun_every_site(&mut snapshot);
        for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
            let graph = build(&snapshot);
            let (allocation, described) = allocate_and_describe(&graph, &Roles::of_abi(abi));
            assert_eq!(verify_allocation(&graph, &allocation, &described), Ok(()));
        }
    }
    let mut snapshot = test_snapshot(vec![
        Instruction::LoadLiteral {
            destination: 13,
            literal: Literal::SmallInteger(1),
        },
        Instruction::LoadSelf { destination: 14 },
        Instruction::IntegerBinary {
            operation: IntegerOperation::Add,
            destination: 15,
            left: 13,
            right: 13,
            target: 4,
        },
        Instruction::Return { register: 15 },
        test_send(13, 13, 15),
        Instruction::Return { register: 15 },
    ]);
    unrun_site(&mut snapshot, 2, 0);
    let graph = build(&snapshot);
    let (allocation, described) = allocate_and_describe(&graph, &Roles::of_abi(Abi::host()));
    assert_eq!(verify_allocation(&graph, &allocation, &described), Ok(()));
    let traps: Vec<&super::DirtyPoint> = allocation
        .dirty_at
        .iter()
        .filter(|point| matches!(point.kind, PointKind::Trap { node: Some(_) }))
        .collect();
    assert_eq!(traps.len(), 1);
    let mut stored: Vec<u16> = traps[0]
        .stores
        .iter()
        .map(|(register, _, _)| *register)
        .collect();
    stored.sort_unstable();
    assert_eq!(stored, vec![13, 14]);
    let point = described
        .points
        .iter()
        .zip(&allocation.dirty_at)
        .find(|(_, dirty)| matches!(dirty.kind, PointKind::Trap { .. }))
        .map(|(point, _)| point)
        .expect("the trap is described");
    assert_eq!(
        (point.scopes[0].program_counter, point.scopes[0].resumes),
        (2, Resumes::Reexecute)
    );
}

/// Every interrupt point of an inlined graph names its chain of scopes,
/// innermost first: the callee's frame node its own scope at its own
/// instruction, its registers from its own register 0 -- its argument,
/// receiver and current class, and none of the header words the exit
/// writes -- then the root at the send it is inlined at, delivering, its
/// registers below the send's slots alone.
#[test]
fn every_interrupt_point_names_its_chain_of_scopes() {
    use crate::jit::optimizing::build::build_inlined;
    use crate::jit::optimizing::build::inlining_tests::a_callee_with_a_frame_node;
    use crate::jit::optimizing::graph::ScopeId;
    use crate::jit::optimizing::plan::plan;
    use crate::jit::optimizing::Speculation;

    let tree = a_callee_with_a_frame_node();
    let plan = plan(&tree, Speculation::Traps);
    let graph = build_inlined(&tree, &plan, Speculation::Traps);
    for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
        let (allocation, described) = allocate_and_describe(&graph, &Roles::of_abi(abi));
        assert_eq!(verify_allocation(&graph, &allocation, &described), Ok(()));
        for point in &described.points {
            let chain: Vec<ScopeId> = point.scopes.iter().map(|scope| scope.scope).collect();
            assert_eq!(chain, graph.chain(point.scopes[0].scope));
            for (inner, outer) in point.scopes.iter().zip(&point.scopes[1..]) {
                assert_eq!(
                    graph.scope(inner.scope).parent,
                    Some((outer.scope, outer.program_counter))
                );
                assert_eq!(outer.resumes, Resumes::Deliver);
            }
        }
        let inside = described
            .points
            .iter()
            .find(|point| point.scopes[0].scope == ScopeId(1))
            .expect("the callee's frame node");
        let [callee, root] = &inside.scopes[..] else {
            panic!("two scopes: {inside:?}");
        };
        assert_eq!(
            callee.code_id,
            tree.callees.keys().copied().next().expect("a callee")
        );
        assert_eq!(
            (callee.program_counter, callee.resumes, callee.offset),
            (0, Resumes::Reexecute, 20)
        );
        let registers: Vec<u16> = callee
            .locations
            .iter()
            .map(|(register, _)| *register)
            .collect();
        // Its argument and its receiver; its current class, which its
        // frame node computes, is the exit's to write.
        assert!(registers.starts_with(&[0, 1]), "{registers:?}");
        assert!(registers.iter().all(|register| !(2..14).contains(register)));
        assert_eq!(
            (root.scope, root.program_counter, root.resumes, root.offset),
            (ScopeId::ROOT, 2, Resumes::Deliver, 0)
        );
        assert!(root.locations.iter().all(|(register, _)| *register < 20));
        assert!(root.locations.iter().any(|(register, _)| *register == 13));
        assert!(inside
            .scopes
            .iter()
            .flat_map(|scope| &scope.locations)
            .all(|(_, location)| *location == Location::FrameWord));
    }
}

/// Every graph the inlining corpus's trees plan, speculating and not,
/// allocates and verifies on every ABI: every scope's registers, its
/// receiver and current class among them, written back at every
/// interrupt point inside it, and the metadata naming each point's chain.
#[test]
fn every_inlined_allocation_verifies() {
    use crate::jit::optimizing::build::build_inlined;
    use crate::jit::optimizing::plan::plan_with;
    use crate::jit::optimizing::{inlining_corpus, Speculation, SWEEP_LIMITS};

    let mut chained = 0;
    for tree in inlining_corpus() {
        for speculation in [Speculation::Traps, Speculation::None] {
            let plan = plan_with(&tree, &SWEEP_LIMITS, speculation);
            let graph = build_inlined(&tree, &plan, speculation);
            for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
                let order = block_order(&graph);
                let allocation = allocate(
                    &graph,
                    &crate::jit::optimizing::liveness::intervals_knowing(&graph, &order),
                    &Roles::of_abi(abi),
                );
                let described = metadata(&graph, &allocation);
                if let Err(error) = verify_allocation(&graph, &allocation, &described) {
                    panic!("{error:?} on {abi:?} in {plan:#?}\n{graph:#?}\n{allocation:#?}");
                }
                chained += described
                    .points
                    .iter()
                    .filter(|point| point.scopes.len() > 1)
                    .count();
            }
        }
    }
    assert!(chained > 0);
}

/// The poll on the back edge to a loop's head that is an unguarded
/// inlined scope's entry names the sender alone, at the send, running it
/// again: that pass has not laid the callee's frame yet.
#[test]
fn a_poll_into_an_unguarded_entry_names_its_sender_at_the_send() {
    use crate::jit::optimizing::build::build_inlined;
    use crate::jit::optimizing::build::inlining_tests::a_loop_headed_by_an_unguarded_send;
    use crate::jit::optimizing::graph::ScopeId;
    use crate::jit::optimizing::plan::plan;
    use crate::jit::optimizing::Speculation;

    let tree = a_loop_headed_by_an_unguarded_send();
    let plan = plan(&tree, Speculation::Traps);
    let graph = build_inlined(&tree, &plan, Speculation::Traps);
    for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
        let (allocation, described) = allocate_and_describe(&graph, &Roles::of_abi(abi));
        assert_eq!(verify_allocation(&graph, &allocation, &described), Ok(()));
        let polls: Vec<_> = allocation
            .dirty_at
            .iter()
            .zip(&described.points)
            .filter(|(point, _)| point.kind == PointKind::Poll)
            .map(|(_, described)| described)
            .collect();
        let [poll] = polls[..] else {
            panic!("one poll: {polls:?}");
        };
        let [root] = &poll.scopes[..] else {
            panic!("the sender alone: {poll:?}");
        };
        assert_eq!(
            (root.scope, root.program_counter, root.resumes),
            (ScopeId::ROOT, 1, Resumes::Reexecute)
        );
    }
}

/// An unguarded inlined scope's entry where two ways meet, one through a
/// block of its own, allocates and verifies: the builder, liveness and
/// the replay read one leftover analysis at its top, its sender's.
#[test]
fn an_unguarded_entry_where_two_ways_meet_allocates_and_verifies() {
    use crate::jit::optimizing::build::build_inlined;
    use crate::jit::optimizing::build::inlining_tests::an_unguarded_send_where_two_ways_meet;
    use crate::jit::optimizing::plan::plan;
    use crate::jit::optimizing::Speculation;

    let tree = an_unguarded_send_where_two_ways_meet();
    for speculation in [Speculation::Traps, Speculation::None] {
        let plan = plan(&tree, speculation);
        let graph = build_inlined(&tree, &plan, speculation);
        for abi in [Abi::Windows64, Abi::SystemV, Abi::Aapcs64] {
            let order = block_order(&graph);
            let allocation = allocate(
                &graph,
                &crate::jit::optimizing::liveness::intervals_knowing(&graph, &order),
                &Roles::of_abi(abi),
            );
            let described = metadata(&graph, &allocation);
            assert_eq!(verify_allocation(&graph, &allocation, &described), Ok(()));
        }
    }
}

/// `r20 := self new: 0. r13 := r20.` then a counting loop over `r13`
/// whose fallbacks never ran, the send's result held in `r20`, a scratch
/// register the leftover analysis taints at the loop's head: the head
/// keeps `r20` bound clean to the send's result, a value no code past the
/// head reads.
fn a_send_result_kept_in_a_tainted_register() -> Snapshot {
    let mut snapshot = test_snapshot(vec![
        Instruction::LoadSelf { destination: 18 },
        Instruction::LoadLiteral {
            destination: 19,
            literal: Literal::SmallInteger(0),
        },
        test_send(18, 19, 20),
        Instruction::Move {
            destination: 13,
            source: 20,
        },
        Instruction::LoadLiteral {
            destination: 15,
            literal: Literal::SmallInteger(1),
        },
        Instruction::IntegerBinary {
            operation: IntegerOperation::Add,
            destination: 16,
            left: 13,
            right: 15,
            target: 7,
        },
        Instruction::Jump { target: 8 },
        test_send(13, 15, 16),
        Instruction::Move {
            destination: 13,
            source: 16,
        },
        Instruction::LoadLiteral {
            destination: 17,
            literal: Literal::SmallInteger(10),
        },
        Instruction::IntegerBinary {
            operation: IntegerOperation::Less,
            destination: 14,
            left: 13,
            right: 17,
            target: 12,
        },
        Instruction::Jump { target: 13 },
        test_send(13, 17, 14),
        Instruction::BranchIfTrue {
            register: 14,
            target: 4,
        },
        Instruction::Return { register: 13 },
    ]);
    // The loop's fallbacks never ran: its addition and comparison trap,
    // and nothing in it writes `r20`.
    unrun_site(&mut snapshot, 5, 1);
    unrun_site(&mut snapshot, 10, 2);
    for tainted in snapshot.tainted_before.iter_mut().skip(4) {
        if !tainted.iter().any(|range| range.contains(&20)) {
            tainted.push(20..21);
        }
    }
    snapshot
}

/// A frame that goes back into its code at the loop's poll finds `nil` in
/// a register the leftover analysis taints at the head, where the
/// collector's pre-pass stored it: the head's clean binding of it is no
/// disagreement while the value it names is read past the head by no one,
/// and is one -- the way back in brings the head a word that does not
/// hold its value -- once the value counts as read there.
#[test]
fn a_poll_s_re_entry_holds_a_tainted_register_to_its_binding_only_while_its_value_is_read() {
    use crate::jit::optimizing::metadata::EVERY_VALUE_LIVE_AT_A_HEAD;

    let snapshot = a_send_result_kept_in_a_tainted_register();
    let (graph, allocation) = allocated(&snapshot, Abi::Windows64);
    let head = graph.loop_heads().next().expect("the body loops");
    let entry = allocation.entries[head.index()]
        .as_ref()
        .expect("the head is in the order");
    assert!(
        entry.bindings[20].clean && entry.bindings[20].value.is_some(),
        "{:?}",
        entry.bindings[20]
    );
    assert_eq!(verified(&graph, &allocation), Ok(()));
    EVERY_VALUE_LIVE_AT_A_HEAD.set(true);
    let refused = verified(&graph, &allocation);
    EVERY_VALUE_LIVE_AT_A_HEAD.set(false);
    assert!(
        matches!(refused, Err(VerifyError::EdgeDisagrees { successor, .. }) if successor == head),
        "{refused:?}"
    );
}

/// An inlined scope's current class, which its frame node's template
/// reads, is written back at that node: left out, the allocation is
/// refused, although the word is the exit's to write at every other point.
#[test]
fn a_frame_node_s_current_class_left_unwritten_is_refused() {
    use crate::jit::optimizing::build::build_inlined;
    use crate::jit::optimizing::build::inlining_tests::a_callee_with_a_frame_node;
    use crate::jit::optimizing::graph::{HeaderBinding, ScopeId};
    use crate::jit::optimizing::plan::plan;
    use crate::jit::optimizing::Speculation;

    let tree = a_callee_with_a_frame_node();
    let plan = plan(&tree, Speculation::Traps);
    let graph = build_inlined(&tree, &plan, Speculation::Traps);
    let order = block_order(&graph);
    let mut allocation = allocate(
        &graph,
        &crate::jit::optimizing::liveness::intervals_knowing(&graph, &order),
        &Roles::of_abi(Abi::Windows64),
    );
    let described = metadata(&graph, &allocation);
    assert_eq!(verify_allocation(&graph, &allocation, &described), Ok(()));
    let class = graph
        .scope(ScopeId(1))
        .header_register(HeaderBinding::CurrentClass);
    let point = allocation
        .dirty_at
        .iter_mut()
        .find(|point| {
            matches!(point.kind, PointKind::Frame { .. })
                && graph.block(point.block).scope == ScopeId(1)
        })
        .expect("the callee's frame node");
    let before = point.stores.len();
    point.stores.retain(|(register, _, _)| *register != class);
    assert!(point.stores.len() < before, "the class is stored there");
    assert!(matches!(
        verify_allocation(&graph, &allocation, &described),
        Err(VerifyError::NotWrittenBack { register, .. }) if register == class
    ));
}
