/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/havlak/LoopTesterApp.java).
 * Changed for ns-next: names are written out, main's parameters and
 * constructControlFlowGraph's loop counters taking the names of
 * benchmarks/are-we-fast-yet/Havlak.ns (dummyLoopCount, parallelLoops,
 * nestedLoops, baseLoops).
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

public final class LoopTesterApp {

  private final ControlFlowGraph   controlFlowGraph;
  private final LoopStructureGraph loopStructureGraph;

  public LoopTesterApp() {
    controlFlowGraph = new ControlFlowGraph();
    loopStructureGraph = new LoopStructureGraph();
    controlFlowGraph.createNode(0);
  }

  // Create 4 basic blocks, corresponding to and if/then/else clause
  // with a CFG that looks like a diamond
  private int buildDiamond(final int start) {
    int basicBlock = start;
    new BasicBlockEdge(controlFlowGraph, basicBlock, basicBlock + 1);
    new BasicBlockEdge(controlFlowGraph, basicBlock, basicBlock + 2);
    new BasicBlockEdge(controlFlowGraph, basicBlock + 1, basicBlock + 3);
    new BasicBlockEdge(controlFlowGraph, basicBlock + 2, basicBlock + 3);

    return basicBlock + 3;
  }

  // Connect two existing nodes
  private void buildConnect(final int start, final int end) {
    new BasicBlockEdge(controlFlowGraph, start, end);
  }

  // Form a straight connected sequence of n basic blocks
  private int buildStraight(final int start, final int length) {
    for (int index = 0; index < length; index++) {
      buildConnect(start + index, start + index + 1);
    }
    return start + length;
  }

  // Construct a simple loop with two diamonds in it
  private int buildBaseLoop(final int from) {
    int header        = buildStraight(from, 1);
    int firstDiamond  = buildDiamond(header);
    int between       = buildStraight(firstDiamond, 1);
    int secondDiamond = buildDiamond(between);
    int footer        = buildStraight(secondDiamond, 1);
    buildConnect(secondDiamond, between);
    buildConnect(firstDiamond, header);

    buildConnect(footer, from);
    footer = buildStraight(footer, 1);
    return footer;
  }

  public int[] main(final int dummyLoopCount, final int findLoopIterations,
      final int parallelLoops, final int nestedLoops, final int baseLoops) {
    constructSimpleControlFlowGraph();
    addDummyLoops(dummyLoopCount);
    constructControlFlowGraph(parallelLoops, nestedLoops, baseLoops);

    // Performing Loop Recognition, 1 Iteration, then findLoopIteration
    findLoops(loopStructureGraph);
    for (int index = 0; index < findLoopIterations; index++) {
      findLoops(new LoopStructureGraph());
    }

    loopStructureGraph.calculateNestingLevel();
    return new int[] {loopStructureGraph.getLoopCount(), controlFlowGraph.getNodeCount()};
  }

  private void constructControlFlowGraph(final int parallelLoops, final int nestedLoops,
      final int baseLoops) {
    int current = 2;

    for (int parallelLoopTree = 0; parallelLoopTree < parallelLoops; parallelLoopTree++) {
      controlFlowGraph.createNode(current + 1);
      buildConnect(2, current + 1);
      current += 1;

      for (int nestedLoop = 0; nestedLoop < nestedLoops; nestedLoop++) {
        int top = current;
        current = buildStraight(current, 1);
        for (int baseLoop = 0; baseLoop < baseLoops; baseLoop++) {
          current = buildBaseLoop(current);
        }
        int bottom = buildStraight(current, 1);
        buildConnect(current, top);
        current = bottom;
      }
      buildConnect(current, 1);
    }
  }

  private void addDummyLoops(final int dummyLoopCount) {
    for (int dummyLoop = 0; dummyLoop < dummyLoopCount; dummyLoop++) {
      findLoops(loopStructureGraph);
    }
  }

  private void findLoops(final LoopStructureGraph loopStructure) {
    HavlakLoopFinder finder = new HavlakLoopFinder(controlFlowGraph, loopStructure);
    finder.findLoops();
  }

  private void constructSimpleControlFlowGraph() {
    controlFlowGraph.createNode(0);
    buildBaseLoop(0);
    controlFlowGraph.createNode(1);
    new BasicBlockEdge(controlFlowGraph, 0, 2);
  }
}
