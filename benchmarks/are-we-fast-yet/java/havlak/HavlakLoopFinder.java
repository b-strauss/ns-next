/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/havlak/HavlakLoopFinder.java).
 * Changed for ns-next: names are written out, as in
 * benchmarks/are-we-fast-yet/newspeak/Havlak.ns, the basic block kinds included
 * (BB_NONHEADER is NON_HEADER, BB_SELF is SELF_LOOP, and so on); the
 * constructor's parameters, the parameters of stepEProcessNonBackPredecessors
 * and the createNewLoop call in findLoops, longer for the names, are split
 * over two lines.
 */
// Copyright 2011 Google Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
package havlak;

import som.IdentityDictionary;
import som.Set;
import som.Vector;

/**
 * The Havlak loop finding algorithm.
 *
 * This class encapsulates the complete finder algorithm
 *
 * @author rhundt
 */
final class HavlakLoopFinder {

  private final ControlFlowGraph   controlFlowGraph;   // Control Flow Graph
  private final LoopStructureGraph loopStructureGraph; // Loop Structure Graph

  // Marker for uninitialized nodes.
  private static final int UNVISITED = Integer.MAX_VALUE;

  // Safeguard against pathological algorithm behavior.
  private static final int MAX_NON_BACK_PREDECESSORS = (32 * 1024);

  private final Vector<Set<Integer>>  nonBackPredecessors = new Vector<Set<Integer>>();
  private final Vector<Vector<Integer>> backPredecessors  = new Vector<>();
  private final IdentityDictionary<BasicBlock, Integer> number = new IdentityDictionary<>();
  private int                      maxSize = 0;
  private int[]                    header;
  private BasicBlockClass[]        type;
  private int[]                    last;
  private UnionFindNode[]          nodes;

  HavlakLoopFinder(final ControlFlowGraph controlFlowGraph,
      final LoopStructureGraph loopStructureGraph) {
    this.controlFlowGraph = controlFlowGraph;
    this.loopStructureGraph = loopStructureGraph;
  }

  /**
   * enum BasicBlockClass
   *
   * Basic Blocks and Loops are being classified as regular, irreducible,
   * and so on. This enum contains a symbolic name for all these classifications
   */
  private enum BasicBlockClass {
    TOP,          // uninitialized
    NON_HEADER,   // a regular BB
    REDUCIBLE,    // reducible loop
    SELF_LOOP,    // single BB loop
    IRREDUCIBLE,  // irreducible loop
    DEAD,         // a dead BB
    LAST          // Sentinel
  }

  //
  // IsAncestor
  //
  // As described in the paper, determine whether a node 'w' is a
  // "true" ancestor for node 'v'.
  //
  // Dominance can be tested quickly using a pre-order trick
  // for depth-first spanning trees. This is why DFS is the first
  // thing we run below.
  //
  private boolean isAncestor(final int ancestor, final int descendant) {
    return ancestor <= descendant && descendant <= last[ancestor];
  }

  //
  // DFS - Depth-First-Search
  //
  // DESCRIPTION:
  // Simple depth first traversal along out edges with node numbering.
  //
  private int depthFirstSearch(final BasicBlock currentNode, final int current) {
    nodes[current].initializeNode(currentNode, current);
    number.atPut(currentNode, current);

    int lastId = current;
    Vector<BasicBlock> outerBlocks = currentNode.getOutEdges();

    for (int index = 0; index < outerBlocks.size(); index++) {
      BasicBlock target = outerBlocks.at(index);
      if (number.at(target) == UNVISITED) {
        lastId = depthFirstSearch(target, lastId + 1);
      }
    }

    last[current] = lastId;
    return lastId;
  }

  private void initializeAllNodes() {
    // Step a:
    //   - initialize all nodes as unvisited.
    //   - depth-first traversal and numbering.
    //   - unreached BB's are marked as dead.
    //
    controlFlowGraph.getBasicBlocks().forEach(
        basicBlock -> number.atPut(basicBlock, UNVISITED));

    depthFirstSearch(controlFlowGraph.getStartBasicBlock(), 0);
  }

  private void identifyEdges(final int size) {
    // Step b:
    //   - iterate over all nodes.
    //
    //   A backedge comes from a descendant in the DFS tree, and non-backedges
    //   from non-descendants (following Tarjan).
    //
    //   - check incoming edges 'v' and add them to either
    //     - the list of backedges (backPreds) or
    //     - the list of non-backedges (nonBackPreds)
    //
    for (int candidate = 0; candidate < size; candidate++) {
      header[candidate] = 0;
      type[candidate] = BasicBlockClass.NON_HEADER;

      BasicBlock candidateBlock = nodes[candidate].getBasicBlock();
      if (candidateBlock == null) {
        type[candidate] = BasicBlockClass.DEAD;
      } else {
        processEdges(candidateBlock, candidate);
      }
    }
  }

  private void processEdges(final BasicBlock candidateBlock, final int candidate) {
    if (candidateBlock.getPredecessorCount() > 0) {
      candidateBlock.getInEdges().forEach(predecessorBlock -> {
        int predecessor = number.at(predecessorBlock);
        if (predecessor != UNVISITED) {
          if (isAncestor(candidate, predecessor)) {
            backPredecessors.at(candidate).append(predecessor);
          } else {
            nonBackPredecessors.at(candidate).add(predecessor);
          }
        }
      });
    }
  }

  //
  // findLoops
  //
  // Find loops and build loop forest using Havlak's algorithm, which
  // is derived from Tarjan. Variable names and step numbering has
  // been chosen to be identical to the nomenclature in Havlak's
  // paper (which, in turn, is similar to the one used by Tarjan).
  //
  public void findLoops() {
    if (controlFlowGraph.getStartBasicBlock() == null) {
      return;
    }

    int size = controlFlowGraph.getNodeCount();

    nonBackPredecessors.removeAll();
    backPredecessors.removeAll();
    number.removeAll();
    if (size > maxSize) {
      header = new int[size];
      type = new BasicBlockClass[size];
      last = new int[size];
      nodes = new UnionFindNode[size];
      maxSize = size;
    }

    for (int index = 0; index < size; ++index) {
      nonBackPredecessors.append(new Set<>());
      backPredecessors.append(new Vector<>());
      nodes[index] = new UnionFindNode();
    }

    initializeAllNodes();
    identifyEdges(size);

    // Start node is root of all other loops.
    header[0] = 0;

    // Step c:
    //
    // The outer loop, unchanged from Tarjan. It does nothing except
    // for those nodes which are the destinations of backedges.
    // For a header node w, we chase backward from the sources of the
    // backedges adding nodes to the set P, representing the body of
    // the loop headed by w.
    //
    // By running through the nodes in reverse of the DFST preorder,
    // we ensure that inner loop headers will be processed before the
    // headers for surrounding loops.
    //
    for (int candidate = size - 1; candidate >= 0; candidate--) {
      // this is 'P' in Havlak's paper
      Vector<UnionFindNode> nodePool = new Vector<>();

      BasicBlock candidateBlock = nodes[candidate].getBasicBlock();
      if (candidateBlock != null) {
        stepD(candidate, nodePool);

        // Copy nodePool to workList.
        //
        Vector<UnionFindNode> workList = new Vector<>();
        nodePool.forEach(each -> workList.append(each));

        if (nodePool.size() != 0) {
          type[candidate] = BasicBlockClass.REDUCIBLE;
        }

        // work the list...
        //
        while (!workList.isEmpty()) {
          UnionFindNode workNode = workList.removeFirst();

          // Step e:
          //
          // Step e represents the main difference from Tarjan's method.
          // Chasing upwards from the sources of a node w's backedges. If
          // there is a node y' that is not a descendant of w, w is marked
          // the header of an irreducible loop, there is another entry
          // into this loop that avoids w.
          //

          // The algorithm has degenerated. Break and
          // return in this case.
          //
          int nonBackSize = nonBackPredecessors.at(workNode.getDepthFirstNumber()).size();
          if (nonBackSize > MAX_NON_BACK_PREDECESSORS) {
            return;
          }
          stepEProcessNonBackPredecessors(candidate, nodePool, workList, workNode);
        }

        // Collapse/Unionize nodes in a SCC to a single node
        // For every SCC found, create a loop descriptor and link it in.
        //
        if ((nodePool.size() > 0) || (type[candidate] == BasicBlockClass.SELF_LOOP)) {
          SimpleLoop loop = loopStructureGraph.createNewLoop(candidateBlock,
              type[candidate] != BasicBlockClass.IRREDUCIBLE);
          setLoopAttributes(candidate, nodePool, loop);
        }
      }
    }  // Step c
  }  // findLoops

  private void stepEProcessNonBackPredecessors(final int candidate,
      final Vector<UnionFindNode> nodePool,
      final Vector<UnionFindNode> workList, final UnionFindNode workNode) {
    nonBackPredecessors.at(workNode.getDepthFirstNumber()).forEach(predecessor -> {
      UnionFindNode predecessorNode = nodes[predecessor];
      UnionFindNode representative = predecessorNode.findSet();

      if (!isAncestor(candidate, representative.getDepthFirstNumber())) {
        type[candidate] = BasicBlockClass.IRREDUCIBLE;
        nonBackPredecessors.at(candidate).add(representative.getDepthFirstNumber());
      } else {
        if (representative.getDepthFirstNumber() != candidate) {
          if (!nodePool.hasSome(each -> each == representative)) {
            workList.append(representative);
            nodePool.append(representative);
          }
        }
      }
    });
  }

  private void setLoopAttributes(final int candidate, final Vector<UnionFindNode> nodePool,
      final SimpleLoop loop) {
    // At this point, one can set attributes to the loop, such as:
    //
    // the bottom node:
    //    iter  = backPreds[w].begin();
    //    loop bottom is: nodes[iter].node);
    //
    // the number of backedges:
    //    backPreds[w].size()
    //
    // whether this loop is reducible:
    //    type[w] != BasicBlockClass.BB_IRREDUCIBLE
    //
    nodes[candidate].setLoop(loop);

    nodePool.forEach(node -> {
      // Add nodes to loop descriptor.
      header[node.getDepthFirstNumber()] = candidate;
      node.union(nodes[candidate]);

      // Nested loops are not added, but linked together.
      if (node.getLoop() != null) {
        node.getLoop().setParent(loop);
      } else {
        loop.addNode(node.getBasicBlock());
      }
    });
  }

  private void stepD(final int candidate, final Vector<UnionFindNode> nodePool) {
    backPredecessors.at(candidate).forEach(predecessor -> {
      if (predecessor != candidate) {
        nodePool.append(nodes[predecessor].findSet());
      } else {
        type[candidate] = BasicBlockClass.SELF_LOOP;
      }
    });
  }
}
