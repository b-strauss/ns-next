/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/cd/Aircraft.java).
 * Changed for ns-next: nothing. Upstream's file carries no licence notice.
 */
package cd;


public class Aircraft {
  public final CallSign callsign;
  public final Vector3D position;

  public Aircraft(final CallSign callsign, final Vector3D position) {
    this.callsign = callsign;
    this.position = position;
  }
}
