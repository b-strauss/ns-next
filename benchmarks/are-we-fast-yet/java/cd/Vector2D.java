/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/cd/Vector2D.java).
 * Changed for ns-next: names are written out, as in
 * benchmarks/are-we-fast-yet/CD.ns (x and y are coordinateX and coordinateY).
 * Upstream's file carries no licence notice.
 */
package cd;


public final class Vector2D implements Comparable<Vector2D> {
  public final double coordinateX;
  public final double coordinateY;

  public Vector2D(final double coordinateX, final double coordinateY) {
    this.coordinateX = coordinateX;
    this.coordinateY = coordinateY;
  }

  public Vector2D plus(final Vector2D other) {
    return new Vector2D(coordinateX + other.coordinateX,
                        coordinateY + other.coordinateY);
  }

  public Vector2D minus(final Vector2D other) {
    return new Vector2D(coordinateX - other.coordinateX,
                        coordinateY - other.coordinateY);
  }

  @Override
  public int compareTo(final Vector2D other) {
    int result = compareNumbers(this.coordinateX, other.coordinateX);
    if (result != 0) {
      return result;
    }
    return compareNumbers(this.coordinateY, other.coordinateY);
  }

  private static int compareNumbers(final double left, final double right) {
    if (left == right) {
      return 0;
    }
    if (left < right) {
      return -1;
    }
    if (left > right) {
      return 1;
    }

    // We say that NaN is smaller than non-NaN.
    if (left == left) {
      return 1;
    }
    return -1;
  }
}
