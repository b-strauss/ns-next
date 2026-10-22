/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/cd/Vector3D.java).
 * Changed for ns-next: names are written out, as in
 * benchmarks/are-we-fast-yet/CD.ns (x, y and z are coordinateX, coordinateY
 * and coordinateZ); the sum in dot, longer for the names, is split over two
 * lines. Upstream's file carries no licence notice.
 */
package cd;


public final class Vector3D {
  public final double coordinateX;
  public final double coordinateY;
  public final double coordinateZ;

  public Vector3D(final double coordinateX, final double coordinateY, final double coordinateZ) {
    this.coordinateX = coordinateX;
    this.coordinateY = coordinateY;
    this.coordinateZ = coordinateZ;
  }

  public Vector3D plus(final Vector3D other) {
    return new Vector3D(coordinateX + other.coordinateX,
                        coordinateY + other.coordinateY,
                        coordinateZ + other.coordinateZ);
  }

  public Vector3D minus(final Vector3D other) {
    return new Vector3D(coordinateX - other.coordinateX,
                        coordinateY - other.coordinateY,
                        coordinateZ - other.coordinateZ);
  }

  public double dot(final Vector3D other) {
    return coordinateX * other.coordinateX + coordinateY * other.coordinateY
        + coordinateZ * other.coordinateZ;
  }

  public double squaredMagnitude() {
    return this.dot(this);
  }

  public double magnitude() {
    return Math.sqrt(squaredMagnitude());
  }

  public Vector3D times(final double amount) {
    return new Vector3D(coordinateX * amount,
                        coordinateY * amount,
                        coordinateZ * amount);
  }
}
