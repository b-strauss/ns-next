//! The checks every graph `build` makes passes (`docs/design/execution.md`,
//! "One tier, an SSA graph, the macro assembler"): a later pass reads a
//! value only where it is defined, and a phi has an input for each way in.
//! The rules apply to the blocks reachable from block 0; an unreachable
//! block never runs.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the optimizing compiler's emitter, Task 5's, is the verifier's first caller"
    )
)]

use super::graph::{Block, BlockId, End, Graph, Node, Place, ValueId};

/// What `verify` refuses in a graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VerifyError {
    /// An edge of `block` names a block the graph does not hold.
    MissingBlock { block: BlockId, target: BlockId },
    /// `block`'s predecessors are not the blocks with an edge to it.
    PredecessorsDisagree { block: BlockId },
    /// `block` ends as no node of it lets it: an `End::Frame` or
    /// `End::FrameJump` after anything but a frame node that ends that
    /// way, or a frame node that does not fall through before its block's
    /// last node.
    NoEnd { block: BlockId },
    /// `value` is defined twice, or not where the value table says.
    DefinedTwice { value: ValueId },
    /// `value`, read in `block`, is defined in no block reachable from
    /// block 0.
    Undefined { value: ValueId, block: BlockId },
    /// `value`, read in `block`, is defined where it does not dominate
    /// the read.
    NotDominated { value: ValueId, block: BlockId },
    /// The phi defining `value` in `block` has not one input per
    /// predecessor.
    PhiInputs { block: BlockId, value: ValueId },
}

/// Checks that every value is defined once, every use is dominated by its
/// definition, every phi has one input per predecessor and every block
/// reachable from block 0 ends as its nodes allow, its edges naming blocks
/// that exist and its predecessors those with an edge to it.
pub(crate) fn verify(graph: &Graph) -> Result<(), VerifyError> {
    let block_count = graph.blocks.len();
    if block_count == 0 {
        return Ok(());
    }
    // Every edge names a block, so that what follows may index by it.
    for (index, block) in graph.blocks.iter().enumerate() {
        for target in block.successors() {
            if target.index() >= block_count {
                return Err(VerifyError::MissingBlock {
                    block: BlockId::of_index(index),
                    target,
                });
            }
        }
    }
    let successors: Vec<Vec<usize>> = graph
        .blocks
        .iter()
        .map(|block| block.successors().into_iter().map(BlockId::index).collect())
        .collect();
    let dominators = immediate_dominators(&successors);
    let reachable = |index: usize| dominators[index].is_some();
    check_predecessors(graph, &successors, &reachable)?;
    for (index, block) in graph.blocks.iter().enumerate() {
        if reachable(index) {
            check_end(block, BlockId::of_index(index))?;
        }
    }
    check_definitions(graph, &reachable)?;
    let dominates = |dominator: usize, block: usize| -> bool {
        let mut block = block;
        loop {
            if block == dominator {
                return true;
            }
            match dominators[block] {
                Some(parent) if parent != block => block = parent,
                _ => return false,
            }
        }
    };
    // Whether `value` may be read in `block` before its node at
    // `position`: a phi of the block precedes every node.
    let available = |value: ValueId, block: usize, position: usize| -> Result<(), VerifyError> {
        let block_id = BlockId::of_index(block);
        let undefined = VerifyError::Undefined {
            value,
            block: block_id,
        };
        let Some(definition) = graph.values.get(value.index()) else {
            return Err(undefined);
        };
        let defining = definition.block.index();
        if defining >= block_count || !reachable(defining) {
            return Err(undefined);
        }
        let before = match definition.place {
            Place::Phi(_) => true,
            Place::Node(index) => index < position,
        };
        if (defining == block && before) || (defining != block && dominates(defining, block)) {
            Ok(())
        } else {
            Err(VerifyError::NotDominated {
                value,
                block: block_id,
            })
        }
    };
    for (index, block) in graph.blocks.iter().enumerate() {
        if !reachable(index) {
            continue;
        }
        let block_id = BlockId::of_index(index);
        for phi in &block.phis {
            if phi.inputs.len() != block.predecessors.len() {
                return Err(VerifyError::PhiInputs {
                    block: block_id,
                    value: phi.value,
                });
            }
            // An input is read where control leaves its predecessor for
            // this block.
            for (input, predecessor) in phi.inputs.iter().zip(&block.predecessors) {
                let leaving = graph
                    .block(*predecessor)
                    .exit_position(block_id)
                    .expect("internal: the predecessors were checked against the edges");
                available(*input, predecessor.index(), leaving)?;
            }
        }
        for (position, node) in block.nodes.iter().enumerate() {
            for value in node.uses() {
                available(value, index, position)?;
            }
        }
        if let Some(value) = block.end.uses() {
            available(value, index, block.nodes.len())?;
        }
    }
    Ok(())
}

/// Each reachable block's predecessors are exactly the reachable blocks
/// with an edge to it, once each.
fn check_predecessors(
    graph: &Graph,
    successors: &[Vec<usize>],
    reachable: &dyn Fn(usize) -> bool,
) -> Result<(), VerifyError> {
    let mut expected: Vec<Vec<usize>> = vec![Vec::new(); graph.blocks.len()];
    for (index, targets) in successors.iter().enumerate() {
        if reachable(index) {
            for target in targets {
                expected[*target].push(index);
            }
        }
    }
    for (index, block) in graph.blocks.iter().enumerate() {
        if !reachable(index) {
            continue;
        }
        let mut listed: Vec<usize> = block.predecessors.iter().map(|id| id.index()).collect();
        let count = listed.len();
        listed.sort_unstable();
        listed.dedup();
        let mut wanted = expected[index].clone();
        wanted.sort_unstable();
        if listed.len() != count || listed != wanted {
            return Err(VerifyError::PredecessorsDisagree {
                block: BlockId::of_index(index),
            });
        }
    }
    Ok(())
}

/// A block's end agrees with its nodes: only a frame node that does not
/// fall through ends a block as `End::Frame`, only one that also jumps (a
/// mirror send) as `End::FrameJump`, and nothing follows a frame node
/// that does not fall through.
fn check_end(block: &Block, block_id: BlockId) -> Result<(), VerifyError> {
    let stops = |node: &Node| matches!(node, Node::Frame { instruction, .. } if !instruction.falls_through());
    let jumps = |node: &Node| matches!(node, Node::Frame { instruction, .. } if instruction.jump_target().is_some());
    let early_stop = block
        .nodes
        .iter()
        .take(block.nodes.len().saturating_sub(1))
        .any(stops);
    let last = block.nodes.last();
    let agrees = match &block.end {
        End::Frame => last.is_some_and(stops),
        End::FrameJump { .. } => last.is_some_and(jumps),
        End::Jump(_) | End::Branch { .. } | End::Return { .. } => !last.is_some_and(stops),
    };
    if agrees && !early_stop {
        Ok(())
    } else {
        Err(VerifyError::NoEnd { block: block_id })
    }
}

/// Every value a reachable block defines is defined once, where the value
/// table says.
fn check_definitions(graph: &Graph, reachable: &dyn Fn(usize) -> bool) -> Result<(), VerifyError> {
    let mut seen = vec![false; graph.values.len()];
    for (index, block) in graph.blocks.iter().enumerate() {
        if !reachable(index) {
            continue;
        }
        let block_id = BlockId::of_index(index);
        let phis = block
            .phis
            .iter()
            .enumerate()
            .map(|(position, phi)| (phi.value, Place::Phi(position)));
        let nodes = block
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(position, node)| Some((node.defined()?, Place::Node(position))));
        for (value, place) in phis.chain(nodes) {
            let Some(definition) = graph.values.get(value.index()) else {
                return Err(VerifyError::DefinedTwice { value });
            };
            if seen[value.index()] || definition.block != block_id || definition.place != place {
                return Err(VerifyError::DefinedTwice { value });
            }
            seen[value.index()] = true;
        }
    }
    Ok(())
}

/// Each block's immediate dominator by the iterative algorithm of Cooper,
/// Harvey and Kennedy ("A Simple, Fast Dominance Algorithm", 2001), over
/// the blocks in reverse postorder from block 0: block 0 is its own, and
/// an unreachable block has none. `successors` lists each block's.
pub(crate) fn immediate_dominators(successors: &[Vec<usize>]) -> Vec<Option<usize>> {
    let count = successors.len();
    let mut dominators: Vec<Option<usize>> = vec![None; count];
    if count == 0 {
        return dominators;
    }
    // The postorder by an explicit stack, since a body's control flow may
    // nest deeper than the native stack would let a recursion go.
    let mut postorder = Vec::with_capacity(count);
    let mut visited = vec![false; count];
    let mut stack = vec![(0usize, 0usize)];
    visited[0] = true;
    while let Some((block, next)) = stack.last_mut() {
        if let Some(&successor) = successors[*block].get(*next) {
            *next += 1;
            if !visited[successor] {
                visited[successor] = true;
                stack.push((successor, 0));
            }
        } else {
            postorder.push(*block);
            stack.pop();
        }
    }
    let mut postorder_number = vec![usize::MAX; count];
    for (number, block) in postorder.iter().enumerate() {
        postorder_number[*block] = number;
    }
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (block, targets) in successors.iter().enumerate() {
        if visited[block] {
            for target in targets {
                predecessors[*target].push(block);
            }
        }
    }
    dominators[0] = Some(0);
    let mut changed = true;
    while changed {
        changed = false;
        for &block in postorder.iter().rev().skip(1) {
            let mut chosen: Option<usize> = None;
            for &predecessor in &predecessors[block] {
                if dominators[predecessor].is_none() {
                    continue;
                }
                chosen = Some(match chosen {
                    None => predecessor,
                    Some(other) => intersect(&dominators, &postorder_number, predecessor, other),
                });
            }
            if chosen.is_some() && dominators[block] != chosen {
                dominators[block] = chosen;
                changed = true;
            }
        }
    }
    dominators
}

/// The nearest common dominator of two blocks whose dominators are known,
/// found by walking up from whichever is lower in postorder.
fn intersect(
    dominators: &[Option<usize>],
    postorder_number: &[usize],
    first: usize,
    second: usize,
) -> usize {
    let mut first = first;
    let mut second = second;
    while first != second {
        while postorder_number[first] < postorder_number[second] {
            first = dominators[first].expect("internal: a processed block has a dominator");
        }
        while postorder_number[second] < postorder_number[first] {
            second = dominators[second].expect("internal: a processed block has a dominator");
        }
    }
    first
}

#[cfg(test)]
mod tests {
    use super::{immediate_dominators, verify, VerifyError};
    use crate::jit::optimizing::graph::{Block, BlockId, End, Graph, Node, Phi, ValueId};

    fn block(program_counter: u32, nodes: Vec<Node>, end: End, predecessors: Vec<u32>) -> Block {
        Block {
            program_counter,
            phis: Vec::new(),
            nodes,
            end,
            predecessors: predecessors.into_iter().map(BlockId).collect(),
            loop_head: false,
        }
    }

    #[test]
    fn a_use_before_its_definition_is_refused() {
        // Block 2 returns a value only block 1, one of its two
        // predecessors, defines.
        let graph = Graph::from_blocks(vec![
            block(
                0,
                vec![Node::Constant {
                    value: ValueId(1),
                    word: 0,
                }],
                End::Branch {
                    condition: ValueId(1),
                    if_true: BlockId(1),
                    if_false: BlockId(2),
                    not_boolean: BlockId(2),
                },
                vec![],
            ),
            block(
                1,
                vec![Node::Constant {
                    value: ValueId(0),
                    word: 0,
                }],
                End::Jump(BlockId(2)),
                vec![0],
            ),
            block(2, vec![], End::Return { value: ValueId(0) }, vec![0, 1]),
        ]);
        assert!(matches!(
            verify(&graph),
            Err(VerifyError::NotDominated { .. })
        ));
    }

    #[test]
    fn a_phi_takes_one_input_per_predecessor() {
        // Block 1 is entered from block 0 and from itself, but its phi
        // has one input.
        let mut join = block(
            1,
            vec![],
            End::Branch {
                condition: ValueId(1),
                if_true: BlockId(1),
                if_false: BlockId(1),
                not_boolean: BlockId(1),
            },
            vec![0, 1],
        );
        join.phis.push(Phi {
            register: 3,
            value: ValueId(1),
            inputs: vec![ValueId(0)],
        });
        let graph = Graph::from_blocks(vec![
            block(
                0,
                vec![Node::Constant {
                    value: ValueId(0),
                    word: 0,
                }],
                End::Jump(BlockId(1)),
                vec![],
            ),
            join,
        ]);
        assert!(matches!(verify(&graph), Err(VerifyError::PhiInputs { .. })));
    }

    #[test]
    fn predecessors_are_the_blocks_with_an_edge_here() {
        let graph = Graph::from_blocks(vec![
            block(
                0,
                vec![Node::Constant {
                    value: ValueId(0),
                    word: 0,
                }],
                End::Jump(BlockId(1)),
                vec![],
            ),
            block(1, vec![], End::Return { value: ValueId(0) }, vec![]),
        ]);
        assert!(matches!(
            verify(&graph),
            Err(VerifyError::PredecessorsDisagree { .. })
        ));
    }

    #[test]
    fn a_loop_s_head_dominates_its_body_and_not_what_follows_its_exit() {
        // 0 -> 1, 4; 1 -> 2, 3; 2 -> 1; 3 -> 4; 5 -> 4, 5 unreachable.
        let successors = vec![vec![1, 4], vec![2, 3], vec![1], vec![4], vec![], vec![4]];
        assert_eq!(
            immediate_dominators(&successors),
            vec![Some(0), Some(0), Some(1), Some(1), Some(0), None]
        );
    }
}
