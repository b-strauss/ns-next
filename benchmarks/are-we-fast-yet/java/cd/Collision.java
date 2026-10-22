/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/cd/Collision.java).
 * Changed for ns-next: nothing. Upstream's file carries no licence notice.
 */
package cd;


public final class Collision {
  public final CallSign aircraftA;
  public final CallSign aircraftB;
  public final Vector3D position;

  public Collision(final CallSign aircraftA, final CallSign aircraftB,
      final Vector3D position) {
    this.aircraftA = aircraftA;
    this.aircraftB = aircraftB;
    this.position = position;
  }
}
