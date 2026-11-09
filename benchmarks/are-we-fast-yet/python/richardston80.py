# Translated for ns-next from `RichardsTon80.ns`, itself adapted from
# Primordial Soup's `Richards.ns`, commit
# `338f0bb342a618f5608ab72ab1b1fae9b226e472`: an OS kernel simulation
# benchmark, originally written in BCPL by Martin Richards, in the version
# derived from the Dart Ton80 one. It is not the Are We Fast Yet Richards,
# which is richards.py beside it. The module's slots are the module-private
# constants below; the checks of both counts, which the port's bench makes,
# are `verify_result`'s.

# Copyright 2008, the Newspeak project authors.
#
# Licensed under the Apache License, Version 2.0 (the ''License''); you
# may not use this file except in compliance with the License. You may
# obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
# and in THIRD-PARTY-NOTICES.md at the repository root.
from abc import abstractmethod

from benchmark import Benchmark

_DATA_SIZE = 4
_IDLE_COUNT = 1000
_IDLE_IDENTITY = 1
_WORKER_IDENTITY = 2
_HANDLER_A_IDENTITY = 3
_HANDLER_B_IDENTITY = 4
_DEVICE_A_IDENTITY = 5
_DEVICE_B_IDENTITY = 6
_NUMBER_OF_IDENTITIES = 6
_KIND_DEVICE = 0
_KIND_WORK = 1

# The task is running and is currently scheduled.
_STATE_RUNNING = 0
# The task has packets left to process.
_STATE_RUNNABLE = 1
# The task is not currently running. The task is not blocked as such and may
# be started by the scheduler.
_STATE_SUSPENDED = 2
# The task is blocked and cannot be run until it is explicitly released.
_STATE_HELD = 4
_STATE_SUSPENDED_RUNNABLE = _STATE_SUSPENDED | _STATE_RUNNABLE
_STATE_NOT_HELD = ~_STATE_HELD


class _Packet:
    """A simple package of data that is manipulated by the tasks.

    The exact layout of the payload data carried by a packet is not
    important, and neither is the nature of the work performed on packets by
    the tasks. Besides carrying data, packets form linked lists and are hence
    used both as data and worklists.
    """

    def __init__(self, link, identity, kind):
        self.link = link  # The tail of the linked list of packets.
        self.identity = identity  # An ID for this packet.
        self.kind = kind  # The type of this packet.
        self.datum = 0
        self.data = [None] * _DATA_SIZE

    def append_to(self, queue_head):
        """Add this packet to the end of a worklist, and return the worklist."""
        self.link = None
        if queue_head is None:
            return self
        next_packet = queue_head
        while (peek := next_packet.link) is not None:
            next_packet = peek
        next_packet.link = self
        return queue_head


class _Task:
    """Abstract task that manipulates work packets."""

    def __init__(self, scheduler):
        self._scheduler = scheduler  # The scheduler that manages this task.

    @abstractmethod
    def run(self, packet):
        pass


class _DeviceTask(_Task):
    """A task that suspends itself after each time it has been run to simulate
    waiting for data from an external device."""

    def __init__(self, scheduler):
        super().__init__(scheduler)
        self._pending = None

    def run(self, packet):
        if packet is None:
            if self._pending is None:
                return self._scheduler.suspend_current()
            function_work = self._pending
            self._pending = None
            return self._scheduler.queue(function_work)
        self._pending = packet
        return self._scheduler.hold_current()


class _HandlerTask(_Task):
    """A task that manipulates work packets and then suspends itself."""

    def __init__(self, scheduler):
        super().__init__(scheduler)
        self._work_in = None
        self._device_in = None

    def run(self, packet):
        if packet is not None:
            if packet.kind == _KIND_WORK:
                self._work_in = packet.append_to(self._work_in)
            else:
                self._device_in = packet.append_to(self._device_in)
        if self._work_in is not None:
            count = self._work_in.datum
            if count < _DATA_SIZE:
                if self._device_in is not None:
                    queued = self._device_in
                    self._device_in = self._device_in.link
                    queued.datum = self._work_in.data[count]
                    self._work_in.datum = count + 1
                    return self._scheduler.queue(queued)
            else:
                queued = self._work_in
                self._work_in = self._work_in.link
                return self._scheduler.queue(queued)
        return self._scheduler.suspend_current()


class _IdleTask(_Task):
    """An idle task doesn't do any work itself but cycles control between the
    two device tasks."""

    def __init__(self, scheduler, control, count):
        super().__init__(scheduler)
        # A seed value that controls how the device tasks are scheduled.
        self._control = control
        # The number of times this task should be scheduled.
        self._count = count

    def run(self, packet):
        self._count -= 1
        if self._count == 0:
            return self._scheduler.hold_current()
        if self._control & 1 == 0:
            self._control //= 2
            return self._scheduler.release(_DEVICE_A_IDENTITY)
        self._control = (self._control // 2) ^ 0xD008
        return self._scheduler.release(_DEVICE_B_IDENTITY)


class _WorkerTask(_Task):
    """A task that manipulates work packets."""

    def __init__(self, scheduler, destination, count):
        super().__init__(scheduler)
        self._destination = destination
        self._count = count

    def run(self, packet):
        if packet is None:
            return self._scheduler.suspend_current()
        if self._destination == _HANDLER_A_IDENTITY:
            self._destination = _HANDLER_B_IDENTITY
        else:
            self._destination = _HANDLER_A_IDENTITY
        packet.identity = self._destination
        packet.datum = 0
        for index in range(_DATA_SIZE):
            self._count += 1
            if self._count > 26:
                self._count = 1
            packet.data[index] = self._count
        return self._scheduler.queue(packet)


class _TaskControlBlock:
    """A task control block manages a task and the queue of work packages
    associated with it."""

    def __init__(self, link, identity, priority, queue, task):
        self.link = link
        self.identity = identity
        self.priority = priority
        self._queue = queue
        self._task = task
        if queue is None:
            self._state = _STATE_SUSPENDED
        else:
            self._state = _STATE_SUSPENDED_RUNNABLE

    def add_input(self, packet, old_task):
        """Adds a packet to the worklist of this block's task, marks this as
        runnable if necessary, and returns the next runnable object to run
        (the one with the highest priority)."""
        if self._queue is None:
            self._queue = packet
            self.mark_as_runnable()
            if self.priority > old_task.priority:
                return self
        else:
            self._queue = packet.append_to(self._queue)
        return old_task

    def is_held_or_suspended(self):
        return (self._state & _STATE_HELD) != 0 or self._state == _STATE_SUSPENDED

    def mark_as_held(self):
        self._state |= _STATE_HELD

    def mark_as_not_held(self):
        self._state &= _STATE_NOT_HELD

    def mark_as_runnable(self):
        self._state |= _STATE_RUNNABLE

    def mark_as_suspended(self):
        self._state |= _STATE_SUSPENDED

    def run(self):
        """Runs this task, if it is ready to be run, and returns the next task
        to run."""
        packet = None
        if self._state == _STATE_SUSPENDED_RUNNABLE:
            packet = self._queue
            self._queue = packet.link
            if self._queue is None:
                self._state = _STATE_RUNNING
            else:
                self._state = _STATE_RUNNABLE
        return self._task.run(packet)

    def set_running(self):
        self._state = _STATE_RUNNING


class _Scheduler:
    """A scheduler can be used to schedule a set of tasks based on their
    relative priorities.

    Scheduling is done by maintaining a list of task control blocks which
    holds tasks and the data queue they are processing.
    """

    def __init__(self):
        self.queue_count = 0
        self.hold_count = 0
        self._current_block = None
        self._current_identity = None
        self._list = None
        self._blocks = [None] * _NUMBER_OF_IDENTITIES

    def add_device_task(self, identity, priority, queue):
        """Add a device task to this scheduler."""
        self._add_task(identity, priority, queue, _DeviceTask(self))

    def add_handler_task(self, identity, priority, queue):
        """Add a handler task to this scheduler."""
        self._add_task(identity, priority, queue, _HandlerTask(self))

    def add_idle_task(self, identity, priority, queue, count):
        """Add an idle task to this scheduler."""
        self._add_running_task(identity, priority, queue, _IdleTask(self, 1, count))

    def _add_running_task(self, identity, priority, queue, task):
        self._add_task(identity, priority, queue, task)
        self._current_block.set_running()

    def _add_task(self, identity, priority, queue, task):
        self._current_block = _TaskControlBlock(
            self._list, identity, priority, queue, task
        )
        self._list = self._current_block
        # The port's identities count from 1.
        self._blocks[identity - 1] = self._current_block

    def add_worker_task(self, identity, priority, queue):
        """Add a work task to this scheduler."""
        self._add_task(
            identity, priority, queue, _WorkerTask(self, _HANDLER_A_IDENTITY, 0)
        )

    def hold_current(self):
        """Block the currently executing task and return the next task control
        block to run.

        The blocked task will not be made runnable until it is explicitly
        released, even if new work is added to it.
        """
        self.hold_count += 1
        self._current_block.mark_as_held()
        return self._current_block.link

    def queue(self, packet):
        """Add the specified packet to the end of the worklist used by the task
        associated with the packet and make the task runnable if it is
        currently suspended."""
        block = self._blocks[packet.identity - 1]
        if block is None:
            return block
        self.queue_count += 1
        packet.link = None
        packet.identity = self._current_identity
        return block.add_input(packet, self._current_block)

    def release(self, identity):
        """Release a task that is currently blocked and return the next block
        to run."""
        block = self._blocks[identity - 1]
        if block is None:
            return block
        block.mark_as_not_held()
        if block.priority > self._current_block.priority:
            return block
        return self._current_block

    def schedule(self):
        """Execute the tasks managed by this scheduler."""
        self._current_block = self._list
        while self._current_block is not None:
            if self._current_block.is_held_or_suspended():
                self._current_block = self._current_block.link
            else:
                self._current_identity = self._current_block.identity
                self._current_block = self._current_block.run()

    def suspend_current(self):
        """Suspend the currently executing task and return the next task
        control block to run. If new work is added to the suspended task it
        will be made runnable."""
        self._current_block.mark_as_suspended()
        return self._current_block


class RichardsTon80(Benchmark):
    def benchmark(self):
        scheduler = _Scheduler()
        scheduler.add_idle_task(_IDLE_IDENTITY, 0, None, _IDLE_COUNT)

        queue = _Packet(None, _WORKER_IDENTITY, _KIND_WORK)
        queue = _Packet(queue, _WORKER_IDENTITY, _KIND_WORK)
        scheduler.add_worker_task(_WORKER_IDENTITY, 1000, queue)

        queue = _Packet(None, _DEVICE_A_IDENTITY, _KIND_DEVICE)
        queue = _Packet(queue, _DEVICE_A_IDENTITY, _KIND_DEVICE)
        queue = _Packet(queue, _DEVICE_A_IDENTITY, _KIND_DEVICE)
        scheduler.add_handler_task(_HANDLER_A_IDENTITY, 2000, queue)

        queue = _Packet(None, _DEVICE_B_IDENTITY, _KIND_DEVICE)
        queue = _Packet(queue, _DEVICE_B_IDENTITY, _KIND_DEVICE)
        queue = _Packet(queue, _DEVICE_B_IDENTITY, _KIND_DEVICE)
        scheduler.add_handler_task(_HANDLER_B_IDENTITY, 3000, queue)

        scheduler.add_device_task(_DEVICE_A_IDENTITY, 4000, None)
        scheduler.add_device_task(_DEVICE_B_IDENTITY, 5000, None)

        scheduler.schedule()
        return scheduler

    def verify_result(self, result):
        return result.queue_count == 2322 and result.hold_count == 928
