/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/richards/Scheduler.java).
 * Changed for ns-next: the tracing, which upstream never switches on, is
 * left out, as benchmarks/are-we-fast-yet/Richards.ns leaves it out;
 * getQueuePacketCount and getHoldCount are added, so that Richards can name
 * both counts in the port's message when start answers false; names are
 * written out (RBObject is RichardsObject, as in that port).
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

import java.util.Arrays;

public class Scheduler extends RichardsObject {
  private TaskControlBlock taskList;
  private TaskControlBlock currentTask;
  private int currentTaskIdentity;
  private final TaskControlBlock[] taskTable;

  private int queuePacketCount;
  private int holdCount;

  public Scheduler() {
    // init scheduler
    queuePacketCount = 0;
    holdCount = 0;
    taskTable = new TaskControlBlock[NUMBER_OF_TYPES];
    Arrays.fill(taskTable, NO_TASK);
    taskList = NO_TASK;
  }


  void createDevice(final int identity, final int priority,
      final Packet workPacket, final TaskState state) {
    DeviceTaskDataRecord data = new DeviceTaskDataRecord();

    createTask(identity, priority, workPacket, state,
       (final Packet workArgument, final RichardsObject wordArgument) -> {
         DeviceTaskDataRecord dataRecord = (DeviceTaskDataRecord) wordArgument;
         Packet functionWork = workArgument;
         if (NO_WORK == functionWork) {
           if (NO_WORK == (functionWork = dataRecord.getPending())) {
             return markWaiting();
           } else {
             dataRecord.setPending(NO_WORK);
             return queuePacket(functionWork);
           }
         } else {
           dataRecord.setPending(functionWork);
           return holdSelf();
         }},
      data);
  }

  void createHandler(final int identity, final int priority,
      final Packet workPaket, final TaskState state) {
    HandlerTaskDataRecord data = new HandlerTaskDataRecord();
    createTask(identity, priority, workPaket, state,
        (final Packet work, final RichardsObject word) -> {
          HandlerTaskDataRecord dataRecord = (HandlerTaskDataRecord) word;
          if (NO_WORK != work) {
            if (WORK_PACKET_KIND == work.getKind()) {
              dataRecord.workInAdd(work);
            } else {
              dataRecord.deviceInAdd(work);
            }
          }

          Packet workPacket;
          if (NO_WORK == (workPacket = dataRecord.workIn())) {
            return markWaiting();
          } else {
            int count = workPacket.getDatum();
            if (count >= Packet.DATA_SIZE) {
              dataRecord.workIn(workPacket.getLink());
              return queuePacket(workPacket);
            } else {
              Packet devicePacket;
              if (NO_WORK == (devicePacket = dataRecord.deviceIn())) {
                return markWaiting();
              } else {
                dataRecord.deviceIn(devicePacket.getLink());
                devicePacket.setDatum(workPacket.getData()[count]);
                workPacket.setDatum(count + 1);
                return queuePacket(devicePacket);
              }
            }
          }
      }, data);
  }

  void createIdler(final int identity, final int priority, final Packet work,
      final TaskState state) {
        IdleTaskDataRecord data = new IdleTaskDataRecord();
        createTask(identity, priority, work, state,
            (final Packet workArgument, final RichardsObject wordArgument) -> {
              IdleTaskDataRecord dataRecord = (IdleTaskDataRecord) wordArgument;
              dataRecord.setCount(dataRecord.getCount() - 1);
              if (0 == dataRecord.getCount()) {
                return holdSelf();
              } else {
                if (0 == (dataRecord.getControl() & 1)) {
                  dataRecord.setControl(dataRecord.getControl() / 2);
                  return release(DEVICE_A);
                } else {
                  dataRecord.setControl((dataRecord.getControl() / 2) ^ 53256);
                  return release(DEVICE_B);
                }
              }
            }, data);
  }

  Packet createPacket(final Packet link, final int identity, final int kind) {
    return new Packet(link, identity, kind);
  }

  void createTask(final int identity, final int priority,
      final Packet work, final TaskState state,
      final ProcessFunction aBlock, final RichardsObject data) {

    TaskControlBlock task = new TaskControlBlock(taskList, identity,
        priority, work, state, aBlock, data);
    taskList = task;
    taskTable[identity] = task;
  }

  void createWorker(final int identity, final int priority,
      final Packet workPaket, final TaskState state) {
    WorkerTaskDataRecord dataRecord = new WorkerTaskDataRecord();
    createTask(identity, priority, workPaket, state,

        (final Packet work, final RichardsObject word) -> {
          WorkerTaskDataRecord data = (WorkerTaskDataRecord) word;
          if (NO_WORK == work) {
            return markWaiting();
          } else {
            data.setDestination((HANDLER_A == data.getDestination()) ? HANDLER_B : HANDLER_A);
            work.setIdentity(data.getDestination());
            work.setDatum(0);
            for (int index = 0; index < Packet.DATA_SIZE; index++) {
              data.setCount(data.getCount() + 1);
              if (data.getCount() > 26) { data.setCount(1); }
              work.getData()[index] = 65 + data.getCount() - 1;
            }
            return queuePacket(work);
          }
        }, dataRecord);
  }

  public boolean start() {
    Packet workQueue;

    createIdler(IDLER, 0, NO_WORK, TaskState.createRunning());
    workQueue = createPacket(NO_WORK, WORKER, WORK_PACKET_KIND);
    workQueue = createPacket(workQueue, WORKER, WORK_PACKET_KIND);

    createWorker(WORKER, 1000, workQueue, TaskState.createWaitingWithPacket());
    workQueue = createPacket(NO_WORK, DEVICE_A, DEVICE_PACKET_KIND);
    workQueue = createPacket(workQueue, DEVICE_A, DEVICE_PACKET_KIND);
    workQueue = createPacket(workQueue, DEVICE_A, DEVICE_PACKET_KIND);

    createHandler(HANDLER_A, 2000, workQueue, TaskState.createWaitingWithPacket());
    workQueue = createPacket(NO_WORK, DEVICE_B, DEVICE_PACKET_KIND);
    workQueue = createPacket(workQueue, DEVICE_B, DEVICE_PACKET_KIND);
    workQueue = createPacket(workQueue, DEVICE_B, DEVICE_PACKET_KIND);

    createHandler(HANDLER_B, 3000, workQueue, TaskState.createWaitingWithPacket());
    createDevice(DEVICE_A, 4000, NO_WORK, TaskState.createWaiting());
    createDevice(DEVICE_B, 5000, NO_WORK, TaskState.createWaiting());

    schedule();

    return queuePacketCount == 23246 && holdCount == 9297;
  }

  public int getQueuePacketCount() { return queuePacketCount; }
  public int getHoldCount() { return holdCount; }

  TaskControlBlock findTask(final int identity) {
    TaskControlBlock task = taskTable[identity];
    if (NO_TASK == task) { throw new RuntimeException("findTask failed"); }
    return task;
  }

  TaskControlBlock holdSelf() {
    holdCount = holdCount + 1;
    currentTask.setTaskHolding(true);
    return currentTask.getLink();
  }

  TaskControlBlock queuePacket(final Packet packet) {
    TaskControlBlock task = findTask(packet.getIdentity());
    if (NO_TASK == task) { return NO_TASK; }

    queuePacketCount = queuePacketCount + 1;

    packet.setLink(NO_WORK);
    packet.setIdentity(currentTaskIdentity);
    return task.addInputAndCheckPriority(packet, currentTask);
  }

  TaskControlBlock release(final int identity) {
    TaskControlBlock task = findTask(identity);
    if (NO_TASK == task) { return NO_TASK; }
    task.setTaskHolding(false);
    if (task.getPriority() > currentTask.getPriority()) {
      return task;
    } else {
      return currentTask;
    }
  }

  TaskControlBlock markWaiting() {
    currentTask.setTaskWaiting(true);
    return currentTask;
  }

  void schedule() {
    currentTask = taskList;
    while (NO_TASK != currentTask) {
      if (currentTask.isTaskHoldingOrWaiting()) {
        currentTask = currentTask.getLink();
      } else {
        currentTaskIdentity = currentTask.getIdentity();
        currentTask = currentTask.runTask();
      }
    }
  }
}
