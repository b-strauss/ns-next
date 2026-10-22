/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/cd/RedBlackTree.java).
 * Changed for ns-next: names are written out, as in
 * benchmarks/are-we-fast-yet/CD.ns. Upstream's file carries no licence
 * notice.
 */
package cd;

import som.ForEachInterface;


public final class RedBlackTree<K extends Comparable<K>, V> {

  Node<K, V> root;

  public RedBlackTree() {
    root = null;
  }

  private enum Color {
    RED, BLACK
  }

  private static <K, V> Node<K, V> treeMinimum(final Node<K, V> node) {
    Node<K, V> current = node;
    while (current.left != null) {
      current = current.left;
    }
    return current;
  }

  private static final class Node<K, V> {

    private final K    key;
    private V value;
    private Node<K, V> left;
    private Node<K, V> right;
    private Node<K, V> parent;
    private Color      color;

    Node(final K key, final V value) {
      this.key = key;
      this.value = value;
      this.left = null;
      this.right = null;
      this.parent = null;
      this.color = Color.RED;
    }

    private Node<K, V> successor() {
      Node<K, V> node = this;
      if (node.right != null) {
        return treeMinimum(node.right);
      }
      Node<K, V> ancestor = node.parent;
      while (ancestor != null && node == ancestor.right) {
        node = ancestor;
        ancestor = ancestor.parent;
      }
      return ancestor;
    }
  }

  public V put(final K key, final V value) {
    InsertResult<K, V> insertionResult = treeInsert(key, value);
    if (!insertionResult.isNewEntry) {
      return insertionResult.oldValue;
    }
    Node<K, V> node = insertionResult.newNode;

    while (node != root && node.parent.color == Color.RED) {
      if (node.parent == node.parent.parent.left) {
        Node<K, V> uncle = node.parent.parent.right;
        if (uncle != null && uncle.color == Color.RED) {
          // Case 1
          node.parent.color = Color.BLACK;
          uncle.color = Color.BLACK;
          node.parent.parent.color = Color.RED;
          node = node.parent.parent;
        } else {
          if (node == node.parent.right) {
            // Case 2
            node = node.parent;
            leftRotate(node);
          }
          // Case 3
          node.parent.color = Color.BLACK;
          node.parent.parent.color = Color.RED;
          rightRotate(node.parent.parent);
        }
      } else {
        // Same as "then" clause with "right" and "left" exchanged.
        Node<K, V> uncle = node.parent.parent.left;
        if (uncle != null && uncle.color == Color.RED) {
          // Case 1
          node.parent.color = Color.BLACK;
          uncle.color = Color.BLACK;
          node.parent.parent.color = Color.RED;
          node = node.parent.parent;
        } else {
          if (node == node.parent.left) {
            // Case 2
            node = node.parent;
            rightRotate(node);
          }
          // Case 3
          node.parent.color = Color.BLACK;
          node.parent.parent.color = Color.RED;
          leftRotate(node.parent.parent);
        }
      }
    }

    root.color = Color.BLACK;
    return null;
  }

  public V remove(final K key) {
    Node<K, V> target = findNode(key);
    if (target == null) {
      return null;
    }

    // Y is the node to be unlinked from the tree.
    Node<K, V> spliced;
    if (target.left == null || target.right == null) {
      spliced = target;
    } else {
      spliced = target.successor();
    }

    // Y is guaranteed to be non-null at this point.
    Node<K, V> child;
    if (spliced.left != null) {
      child = spliced.left;
    } else {
      child = spliced.right;
    }

    // X is the child of y which might potentially replace y in the tree. X might be null at
    // this point.
    Node<K, V> childParent;
    if (child != null) {
      child.parent = spliced.parent;
      childParent = child.parent;
    } else {
      childParent = spliced.parent;
    }
    if (spliced.parent == null) {
      root = child;
    } else {
      if (spliced == spliced.parent.left) {
        spliced.parent.left = child;
      } else {
        spliced.parent.right = child;
      }
    }

    if (spliced != target) {
      if (spliced.color == Color.BLACK) {
        removeFixup(child, childParent);
      }

      spliced.parent = target.parent;
      spliced.color = target.color;
      spliced.left = target.left;
      spliced.right = target.right;

      if (target.left != null) {
        target.left.parent = spliced;
      }
      if (target.right != null) {
        target.right.parent = spliced;
      }
      if (target.parent != null) {
        if (target.parent.left == target) {
          target.parent.left = spliced;
        } else {
          target.parent.right = spliced;
        }
      } else {
        root = spliced;
      }
    } else if (spliced.color == Color.BLACK) {
      removeFixup(child, childParent);
    }

    return target.value;
  }

  public V get(final K key) {
    Node<K, V> node = findNode(key);
    if (node == null) {
      return null;
    }
    return node.value;
  }

  public static final class Entry<K, V> {
    public final K key;
    public final V value;
    public Entry(final K key, final V value) {
      this.key = key;
      this.value = value;
    }
  }

  public void forEach(final ForEachInterface<Entry<K, V>> function) {
    if (root == null) {
      return;
    }
    Node<K, V> current = treeMinimum(root);
    while (current != null) {
      function.apply(new Entry<>(current.key, current.value));
      current = current.successor();
    }
  }

  private Node<K, V> findNode(final K key) {
    Node<K, V> current = root;
    while (current != null) {
      int comparisonResult = key.compareTo(current.key);
      if (comparisonResult == 0) {
        return current;
      }
      if (comparisonResult < 0) {
        current = current.left;
      } else {
        current = current.right;
      }
    }
    return null;
  }

  private static final class InsertResult<K, V> {
    public final boolean isNewEntry;
    public final Node<K, V> newNode;
    public final V oldValue;

    InsertResult(final boolean isNewEntry, final Node<K, V> newNode, final V oldValue) {
      this.isNewEntry = isNewEntry;
      this.newNode = newNode;
      this.oldValue = oldValue;
    }
  }

  private InsertResult<K, V> treeInsert(final K key, final V value) {
    Node<K, V> parent = null;
    Node<K, V> current = root;

    while (current != null) {
      parent = current;
      int comparisonResult = key.compareTo(current.key);
      if (comparisonResult < 0) {
        current = current.left;
      } else if (comparisonResult > 0) {
        current = current.right;
      } else {
        V oldValue = current.value;
        current.value = value;
        return new InsertResult<>(false, null, oldValue);
      }
    }

    Node<K, V> newNode = new Node<>(key, value);
    newNode.parent = parent;
    if (parent == null) {
      root = newNode;
    } else {
      if (key.compareTo(parent.key) < 0) {
        parent.left = newNode;
      } else {
        parent.right = newNode;
      }
    }
    return new InsertResult<>(true, newNode, null);
  };

  private Node<K, V> leftRotate(final Node<K, V> node) {
    Node<K, V> pivot = node.right;

    // Turn y's left subtree into x's right subtree.
    node.right = pivot.left;
    if (pivot.left != null) {
      pivot.left.parent = node;
    }

    // Link x's parent to y.
    pivot.parent = node.parent;
    if (node.parent == null) {
      root = pivot;
    } else {
      if (node == node.parent.left) {
        node.parent.left = pivot;
      } else {
        node.parent.right = pivot;
      }
    }

    // Put x on y's left.
    pivot.left = node;
    node.parent = pivot;

    return pivot;
  }

  private Node<K, V> rightRotate(final Node<K, V> node) {
    Node<K, V> pivot = node.left;

    // Turn x's right subtree into y's left subtree.
    node.left = pivot.right;
    if (pivot.right != null) {
      pivot.right.parent = node;
    }

    // Link y's parent to x;
    pivot.parent = node.parent;
    if (node.parent == null) {
      root = pivot;
    } else {
      if (node == node.parent.left) {
        node.parent.left = pivot;
      } else {
        node.parent.right = pivot;
      }
    }

    pivot.right = node;
    node.parent = pivot;

    return pivot;
  }

  private void removeFixup(Node<K, V> child, Node<K, V> childParent) {
    while (child != root && (child == null || child.color == Color.BLACK)) {
      if (child == childParent.left) {
        // Note: the text points out that w cannot be null. The reason is not obvious from
        // simply looking at the code; it comes about from the properties of the red-black
        // tree.
        Node<K, V> sibling = childParent.right;
        if (sibling.color == Color.RED) {
          // Case 1
          sibling.color = Color.BLACK;
          childParent.color = Color.RED;
          leftRotate(childParent);
          sibling = childParent.right;
        }
        if ((sibling.left == null || sibling.left.color == Color.BLACK)
            && (sibling.right == null || sibling.right.color == Color.BLACK)) {
          // Case 2
          sibling.color = Color.RED;
          child = childParent;
          childParent = child.parent;
        } else {
          if (sibling.right == null || sibling.right.color == Color.BLACK) {
            // Case 3
            sibling.left.color = Color.BLACK;
            sibling.color = Color.RED;
            rightRotate(sibling);
            sibling = childParent.right;
          }
          // Case 4
          sibling.color = childParent.color;
          childParent.color = Color.BLACK;
          if (sibling.right != null) {
            sibling.right.color = Color.BLACK;
          }
          leftRotate(childParent);
          child = root;
          childParent = child.parent;
        }
      } else {
        // Same as "then" clause with "right" and "left" exchanged.
        Node<K, V> sibling = childParent.left;
        if (sibling.color == Color.RED) {
          // Case 1
          sibling.color = Color.BLACK;
          childParent.color = Color.RED;
          rightRotate(childParent);
          sibling = childParent.left;
        }
        if ((sibling.right == null || sibling.right.color == Color.BLACK)
            && (sibling.left == null || sibling.left.color == Color.BLACK)) {
          // Case 2
          sibling.color = Color.RED;
          child = childParent;
          childParent = child.parent;
        } else {
          if (sibling.left == null || sibling.left.color == Color.BLACK) {
            // Case 3
            sibling.right.color = Color.BLACK;
            sibling.color = Color.RED;
            leftRotate(sibling);
            sibling = childParent.left;
          }
          // Case 4
          sibling.color = childParent.color;
          childParent.color = Color.BLACK;
          if (sibling.left != null) {
            sibling.left.color = Color.BLACK;
          }
          rightRotate(childParent);
          child = root;
          childParent = child.parent;
        }
      }
    }
    if (child != null) {
      child.color = Color.BLACK;
    }
  }
}
