/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/richards/WorkerTaskDataRecord.java).
 * Changed for ns-next: names are written out (RBObject is RichardsObject, as
 * in benchmarks/are-we-fast-yet/Richards.ns).
 */
/*
 * This benchmark is derived from Mario Wolczko's Java and Smalltalk version of
 * Richards.
 * 
 * It is modified based on the SOM version and to use Java 8 features.
 * License details:
 *   http://web.archive.org/web/20050825101121/http://www.sunlabs.com/people/mario/java_benchmarking/index.html
 */
package richards;


final class WorkerTaskDataRecord extends RichardsObject {
  private int destination;
  private int count;

  WorkerTaskDataRecord() {
    destination = HANDLER_A;
    count = 0;
  }

  public int getCount() { return count; }
  public void setCount(final int aCount) { count = aCount; }

  public int getDestination() { return destination; }
  public void setDestination(final int aHandler) { destination = aHandler; }
}
