# Written for ns-next after `Richards.ns`, the Are We Fast Yet Richards
# beside it, and its Java counterpart `java/Richards.java` with
# `java/richards/`: the same tasks, packets, linked lists and scheduler, in
# idiomatic Python. The tracing, which Are We Fast Yet never switches on, is
# left out, as `Richards.ns` leaves it out. `Richards.ns`'s
# RichardsBenchmarks is `_Scheduler`, as in the Java version; the check
# `start` makes of both counts is `verify_result`'s.

# This benchmark is derived from Mario Wolczko's Java and Smalltalk version of
# Richards.
#
# It is modified based on the SOM version and to use Java 8 features.
# License details:
#   http://web.archive.org/web/20050825101121/http://www.sunlabs.com/people/mario/java_benchmarking/index.html
from benchmark import Benchmark

_IDLER = 1
_WORKER = 2
_HANDLER_A = 3
_HANDLER_B = 4
_DEVICE_A = 5
_DEVICE_B = 6
_NUMBER_OF_TASKS = 6

_DEVICE_PACKET_KIND = 1
_WORK_PACKET_KIND = 2

_DATA_SIZE = 4


class _RichardsObject:
    @staticmethod
    def append(packet, queue_head):
        packet.link = None
        if queue_head is None:
            return packet

        mouse = queue_head
        while (link := mouse.link) is not None:
            mouse = link
        mouse.link = packet
        return queue_head


class _DeviceTaskDataRecord:
    def __init__(self):
        self.pending = None


class _HandlerTaskDataRecord(_RichardsObject):
    def __init__(self):
        self.work_in = None
        self.device_in = None

    def device_in_add(self, packet):
        self.device_in = self.append(packet, self.device_in)

    def work_in_add(self, packet):
        self.work_in = self.append(packet, self.work_in)


class _IdleTaskDataRecord:
    def __init__(self):
        self.control = 1
        self.count = 10000


class _WorkerTaskDataRecord:
    def __init__(self):
        self.destination = _HANDLER_A
        self.count = 0


class _Packet:
    def __init__(self, link, identity, kind):
        self.link = link
        self.identity = identity
        self.kind = kind
        self.datum = 1
        self.data = [0] * _DATA_SIZE


class _TaskState(_RichardsObject):
    def __init__(self):
        self.packet_pending = None
        self.task_waiting = None
        self.task_holding = None

    @classmethod
    def running(cls):
        state = cls()
        state.make_running()
        return state

    @classmethod
    def waiting(cls):
        state = cls()
        state.make_waiting()
        return state

    @classmethod
    def waiting_with_packet(cls):
        state = cls()
        state.make_waiting_with_packet()
        return state

    def make_packet_pending(self):
        self.packet_pending = True
        self.task_waiting = False
        self.task_holding = False

    def make_running(self):
        self.packet_pending = self.task_waiting = self.task_holding = False

    def make_waiting(self):
        self.packet_pending = self.task_holding = False
        self.task_waiting = True

    def make_waiting_with_packet(self):
        self.task_holding = False
        self.task_waiting = self.packet_pending = True

    def is_running(self):
        return (
            not self.packet_pending
            and not self.task_waiting
            and not self.task_holding
        )

    def is_task_holding_or_waiting(self):
        return self.task_holding or (not self.packet_pending and self.task_waiting)

    def is_waiting(self):
        return not self.packet_pending and self.task_waiting and not self.task_holding

    def is_waiting_with_packet(self):
        return self.packet_pending and self.task_waiting and not self.task_holding


class _TaskControlBlock(_TaskState):
    def __init__(
        self,
        link,
        identity,
        priority,
        initial_work_queue,
        initial_state,
        function,
        private_data,
    ):
        super().__init__()
        self.link = link
        self.identity = identity
        self.function = function
        self.priority = priority
        self._input = initial_work_queue
        self._handle = private_data
        self.packet_pending = initial_state.packet_pending
        self.task_waiting = initial_state.task_waiting
        self.task_holding = initial_state.task_holding

    def add_input(self, packet, old_task):
        if self._input is None:
            self._input = packet
            self.packet_pending = True
            if self.priority > old_task.priority:
                return self
        else:
            self._input = self.append(packet, self._input)
        return old_task

    def run_task(self):
        if self.is_waiting_with_packet():
            message = self._input
            self._input = message.link
            if self._input is None:
                self.make_running()
            else:
                self.make_packet_pending()
        else:
            message = None
        return self.function(message, self._handle)


class _Scheduler:
    def __init__(self):
        self._task_list = None
        self._current_task = None
        self._current_task_identity = None
        self._task_table = [None] * _NUMBER_OF_TASKS
        self.queue_packet_count = 0
        self.hold_count = 0

    def _create_device(self, identity, priority, work, state):
        def function(packet, record):
            function_work = packet
            if function_work is None:
                function_work = record.pending
                if function_work is None:
                    return self._wait()
                record.pending = None
                return self._queue_packet(function_work)
            record.pending = function_work
            return self._hold_self()

        self._create_task(
            identity, priority, work, state, function, _DeviceTaskDataRecord()
        )

    def _create_handler(self, identity, priority, work, state):
        def function(packet, record):
            if packet is not None:
                if packet.kind == _WORK_PACKET_KIND:
                    record.work_in_add(packet)
                else:
                    record.device_in_add(packet)

            work_packet = record.work_in
            if work_packet is None:
                return self._wait()

            count = work_packet.datum
            if count > _DATA_SIZE:
                record.work_in = work_packet.link
                return self._queue_packet(work_packet)

            device_packet = record.device_in
            if device_packet is None:
                return self._wait()

            record.device_in = device_packet.link
            # `Richards.ns` counts the data from 1, the datum with it.
            device_packet.datum = work_packet.data[count - 1]
            work_packet.datum = count + 1
            return self._queue_packet(device_packet)

        self._create_task(
            identity, priority, work, state, function, _HandlerTaskDataRecord()
        )

    def _create_idler(self, identity, priority, work, state):
        def function(packet, record):
            record.count -= 1
            if record.count == 0:
                return self._hold_self()
            if record.control & 1 == 0:
                record.control //= 2
                return self._release(_DEVICE_A)
            record.control = (record.control // 2) ^ 53256
            return self._release(_DEVICE_B)

        self._create_task(
            identity, priority, work, state, function, _IdleTaskDataRecord()
        )

    def _create_worker(self, identity, priority, work, state):
        def function(packet, record):
            if packet is None:
                return self._wait()

            if record.destination == _HANDLER_A:
                record.destination = _HANDLER_B
            else:
                record.destination = _HANDLER_A
            packet.identity = record.destination
            packet.datum = 1
            for index in range(_DATA_SIZE):
                record.count += 1
                if record.count > 26:
                    record.count = 1
                # 65 is the character code of "A".
                packet.data[index] = 65 + record.count - 1
            return self._queue_packet(packet)

        self._create_task(
            identity, priority, work, state, function, _WorkerTaskDataRecord()
        )

    def _create_task(self, identity, priority, work, state, function, data):
        task = _TaskControlBlock(
            self._task_list, identity, priority, work, state, function, data
        )
        self._task_list = task
        self._task_table[identity - 1] = task

    def start(self):
        self._create_idler(_IDLER, 0, None, _TaskState.running())
        work_queue = _Packet(None, _WORKER, _WORK_PACKET_KIND)
        work_queue = _Packet(work_queue, _WORKER, _WORK_PACKET_KIND)
        self._create_worker(
            _WORKER, 1000, work_queue, _TaskState.waiting_with_packet()
        )

        work_queue = _Packet(None, _DEVICE_A, _DEVICE_PACKET_KIND)
        work_queue = _Packet(work_queue, _DEVICE_A, _DEVICE_PACKET_KIND)
        work_queue = _Packet(work_queue, _DEVICE_A, _DEVICE_PACKET_KIND)
        self._create_handler(
            _HANDLER_A, 2000, work_queue, _TaskState.waiting_with_packet()
        )

        work_queue = _Packet(None, _DEVICE_B, _DEVICE_PACKET_KIND)
        work_queue = _Packet(work_queue, _DEVICE_B, _DEVICE_PACKET_KIND)
        work_queue = _Packet(work_queue, _DEVICE_B, _DEVICE_PACKET_KIND)
        self._create_handler(
            _HANDLER_B, 3000, work_queue, _TaskState.waiting_with_packet()
        )

        self._create_device(_DEVICE_A, 4000, None, _TaskState.waiting())
        self._create_device(_DEVICE_B, 5000, None, _TaskState.waiting())

        self._schedule()

    def _find_task(self, identity):
        task = self._task_table[identity - 1]
        if task is None:
            raise RuntimeError(f"find_task failed for task {identity}")
        return task

    def _hold_self(self):
        self.hold_count += 1
        self._current_task.task_holding = True
        return self._current_task.link

    def _queue_packet(self, packet):
        task = self._find_task(packet.identity)
        # Never true, since _find_task raises first; tested all the same, as
        # `Richards.ns` tests it, so that every port does the same work.
        if task is None:
            return None
        self.queue_packet_count += 1
        packet.link = None
        packet.identity = self._current_task_identity
        return task.add_input(packet, self._current_task)

    def _release(self, identity):
        task = self._find_task(identity)
        if task is None:
            return None
        task.task_holding = False
        if task.priority > self._current_task.priority:
            return task
        return self._current_task

    def _wait(self):
        self._current_task.task_waiting = True
        return self._current_task

    def _schedule(self):
        self._current_task = self._task_list
        while self._current_task is not None:
            if self._current_task.is_task_holding_or_waiting():
                self._current_task = self._current_task.link
            else:
                self._current_task_identity = self._current_task.identity
                self._current_task = self._current_task.run_task()


class Richards(Benchmark):
    def benchmark(self):
        scheduler = _Scheduler()
        scheduler.start()
        return scheduler

    def verify_result(self, result):
        return result.queue_packet_count == 23246 and result.hold_count == 9297
