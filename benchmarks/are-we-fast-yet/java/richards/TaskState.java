/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/richards/TaskState.java).
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


class TaskState extends RichardsObject {
  private boolean packetPending;
  private boolean taskWaiting;
  private boolean taskHolding;

  public boolean isPacketPending() { return packetPending; }
  public boolean isTaskHolding()   { return taskHolding;   }
  public boolean isTaskWaiting()   { return taskWaiting;   }

  public void setTaskHolding(final boolean value) { taskHolding = value; }
  public void setTaskWaiting(final boolean value) { taskWaiting = value; }
  public void setPacketPending(final boolean value) { packetPending = value; }

  public void packetPending() {
    packetPending = true;
    taskWaiting   = false;
    taskHolding   = false;
  }

  public void running() {
    packetPending = taskWaiting = taskHolding = false;
  }

  public void waiting() {
    packetPending = taskHolding = false;
    taskWaiting = true;
  }

  public void waitingWithPacket() {
    taskHolding = false;
    taskWaiting = packetPending = true;
  }

  public boolean isTaskHoldingOrWaiting() {
    return taskHolding || (!packetPending && taskWaiting);
  }

  public boolean isWaitingWithPacket() {
    return packetPending && taskWaiting && !taskHolding;
  }

  public static TaskState createRunning() {
    TaskState state = new TaskState();
    state.running();
    return state;
  }

  public static TaskState createWaiting() {
    TaskState state = new TaskState();
    state.waiting();
    return state;
  }

  public static TaskState createWaitingWithPacket() {
    TaskState state = new TaskState();
    state.waitingWithPacket();
    return state;
  }
}
