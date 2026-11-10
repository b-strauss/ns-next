/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/DeltaBlue.java).
 * Changed for ns-next: innerBenchmarkLoop, benchmark and verifyResult are
 * bench, which runs chainTest and projectionTest once at the size of
 * benchmarks/are-we-fast-yet/newspeak/DeltaBlue.ns, 12000; their checks, in
 * deltablue/Planner.java, are the benchmark's own, as in that port; the class
 * is final. That port's other departures answer Primordial Soup's protocol
 * and have no counterpart here.
 */
/*
 * This benchmark is derived from Mario Wolczko's Java and Smalltalk version of
 * DeltaBlue.
 * 
 * It is modified to use the SOM class library and Java 8 features.
 * License details:
 *   http://web.archive.org/web/20050825101121/http://www.sunlabs.com/people/mario/java_benchmarking/index.html
 */
import deltablue.Planner;

public final class DeltaBlue extends Benchmark {

  @Override
  public void bench() {
    Planner.chainTest(12000);
    Planner.projectionTest(12000);
  }
}
