/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/cd/Constants.java).
 * Changed for ns-next: nothing. Upstream's file carries no licence notice.
 */
package cd;


public final class Constants {
  public static final double MIN_X = 0.0;
  public static final double MIN_Y = 0.0;
  public static final double MAX_X = 1000.0;
  public static final double MAX_Y = 1000.0;
  public static final double MIN_Z = 0.0;
  public static final double MAX_Z = 10.0;
  public static final double PROXIMITY_RADIUS = 1.0;
  public static final double GOOD_VOXEL_SIZE  = PROXIMITY_RADIUS * 2.0;
}
