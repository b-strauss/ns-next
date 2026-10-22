/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/havlak/BasicBlockEdge.java).
 * Changed for ns-next: names are written out.
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

/**
 * A simple class simulating the concept of Edges
 * between Basic Blocks.
 *
 * @author rhundt
 */
final class BasicBlockEdge {

  private final BasicBlock fromBlock;
  private final BasicBlock toBlock;

  BasicBlockEdge(final ControlFlowGraph controlFlowGraph, final int fromName, final int toName) {
    fromBlock = controlFlowGraph.createNode(fromName);
    toBlock   = controlFlowGraph.createNode(toName);

    fromBlock.addOutEdge(toBlock);
    toBlock.addInEdge(fromBlock);

    controlFlowGraph.addEdge(this);
  }
}
