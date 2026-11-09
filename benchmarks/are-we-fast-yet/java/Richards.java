/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Java/src/Richards.java).
 * Changed for ns-next: benchmark and verifyResult are bench, which runs once
 * and checks its own result, with the message of
 * benchmarks/are-we-fast-yet/newspeak/Richards.ns, which names both counts; the class
 * is final.
 */
/*
 * This benchmark is derived from Mario Wolczko's Java and Smalltalk version of
 * Richards.
 *
 * It is modified based on the SOM version and to use Java 8 features.
 * License details:
 *   http://web.archive.org/web/20050825101121/http://www.sunlabs.com/people/mario/java_benchmarking/index.html
 */
import richards.Scheduler;

/*
 * This version is a port of the SOM Richards benchmark to Java.
 * It is kept as close to the SOM version as possible, for cross-language
 * benchmarking.
 */

public final class Richards extends Benchmark {

  @Override
  public void bench() {
    Scheduler scheduler = new Scheduler();
    if (!scheduler.start()) {
      throw new IllegalStateException(
          "Richards: expected 23246 queued packets and 9297 holds, got "
          + scheduler.getQueuePacketCount() + " and " + scheduler.getHoldCount());
    }
  }
}
