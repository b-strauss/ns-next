/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/richards/HandlerTaskDataRecord.java).
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


final class HandlerTaskDataRecord extends RichardsObject {
  private Packet workIn;
  private Packet deviceIn;

  HandlerTaskDataRecord() {
    workIn = deviceIn = NO_WORK;
  }

  public Packet deviceIn() { return deviceIn; }
  public void deviceIn(final Packet aPacket) { deviceIn = aPacket; }

  public void deviceInAdd(final Packet packet) {
    deviceIn = append(packet, deviceIn);
  }

  public Packet workIn() { return workIn; }
  public void workIn(final Packet aWorkQueue) { workIn = aWorkQueue; }

  public void workInAdd(final Packet packet) {
    workIn = append(packet, workIn);
  }
}
