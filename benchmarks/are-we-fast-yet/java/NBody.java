/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Java/src/NBody.java).
 * Changed for ns-next: innerBenchmarkLoop, benchmark and verifyResult are
 * bench, which runs once at benchmarks/are-we-fast-yet/NBody.ns's size,
 * 250000 steps, and checks its energy, -0.1690859889909308, with that port's
 * message; the check for upstream's other size, 1, is dropped; names are
 * written out. Upstream's file carries no licence notice; its comment
 * follows as it is.
 */
/* The Computer Language Benchmarks Game
 * http://shootout.alioth.debian.org/
 *
 * Based on nbody.java and adapted basde on the SOM version.
 */
import nbody.NBodySystem;

public final class NBody extends Benchmark {

  @Override
  public void bench() {
    NBodySystem system = new NBodySystem();
    for (int step = 0; step < 250000; step++) {
      system.advance(0.01);
    }

    double energy = system.energy();
    if (energy != -0.1690859889909308) {
      throw new IllegalStateException(
          "NBody: expected -0.1690859889909308, got " + energy);
    }
  }
}
