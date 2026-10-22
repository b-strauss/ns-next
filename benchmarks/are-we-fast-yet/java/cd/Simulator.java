/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/cd/Simulator.java).
 * Changed for ns-next: names are written out. Upstream's file carries no
 * licence notice.
 */
package cd;

import som.Vector;


public final class Simulator {
  private final Vector<CallSign> aircraft;

  public Simulator(final int aircraftCount) {
    aircraft = new Vector<>();
    for (int index = 0; index < aircraftCount; index++) {
      aircraft.append(new CallSign(index));
    }
  }

  public Vector<Aircraft> simulate(final double time) {
    Vector<Aircraft> frame = new Vector<>();
    for (int index = 0; index < aircraft.size(); index += 2) {
      frame.append(new Aircraft(aircraft.at(index),
          new Vector3D(time, Math.cos(time) * 2 + index * 3, 10)));
      frame.append(new Aircraft(aircraft.at(index + 1),
          new Vector3D(time, Math.sin(time) * 2 + index * 3, 10)));
    }
    return frame;
  }
}
