/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/cd/CallSign.java).
 * Changed for ns-next: nothing. Upstream's file carries no licence notice.
 */
package cd;


public class CallSign implements Comparable<CallSign> {
  private final int value;

  public CallSign(final int value) {
    this.value = value;
  }

  @Override
  public int compareTo(final CallSign other) {
    return (value == other.value) ? 0 : ((value < other.value) ? -1 : 1);
  }
}
