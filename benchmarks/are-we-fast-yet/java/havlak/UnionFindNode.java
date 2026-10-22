/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/havlak/UnionFindNode.java).
 * Changed for ns-next: names are written out. Upstream's file carries no
 * licence notice; its comment follows the package declaration as it is.
 */
package havlak;

import som.Vector;


/**
 * class UnionFindNode
 *
 * The algorithm uses the Union/Find algorithm to collapse
 * complete loops into a single node. These nodes and the
 * corresponding functionality are implemented with this class
 */
final class UnionFindNode {

  private UnionFindNode parent;
  private BasicBlock    basicBlock;
  private SimpleLoop    loop;
  private int           depthFirstNumber;

  UnionFindNode() { }

  // Initialize this node.
  //
  public void initializeNode(final BasicBlock basicBlock, final int depthFirstNumber) {
    this.parent           = this;
    this.basicBlock       = basicBlock;
    this.depthFirstNumber = depthFirstNumber;
    this.loop             = null;
  }

  // Union/Find Algorithm - The find routine.
  //
  // Implemented with Path Compression (inner loops are only
  // visited and collapsed once, however, deep nests would still
  // result in significant traversals).
  //
  public UnionFindNode findSet() {
    Vector<UnionFindNode> nodeList = new Vector<>();

    UnionFindNode node = this;
    while (node != node.parent) {
      if (node.parent != node.parent.parent) {
        nodeList.append(node);
      }
      node = node.parent;
    }

    // Path Compression, all nodes' parents point to the 1st level parent.
    nodeList.forEach(each -> each.union(parent));
    return node;
  }

  // Union/Find Algorithm - The union routine.
  //
  // Trivial. Assigning parent pointer is enough,
  // we rely on path compression.
  //
  public void union(final UnionFindNode basicBlock) {
    parent = basicBlock;
  }

  // Getters/Setters
  //
  public BasicBlock getBasicBlock() {
    return basicBlock;
  }

  public SimpleLoop getLoop() {
    return loop;
  }

  public int getDepthFirstNumber() {
    return depthFirstNumber;
  }

  public void setLoop(final SimpleLoop loop) {
    this.loop = loop;
  }
}
