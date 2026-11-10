# Adapted for ns-next from Are We Fast Yet
# (https://github.com/smarr/are-we-fast-yet, commit
# 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Python/list.py).
# Changed for ns-next: benchmark and verify_result are bench, which runs once
# and checks its own result, raising a RuntimeError with the message of
# benchmarks/are-we-fast-yet/newspeak/List.ns; names are written out.

# This code is based on the SOM class library.
#
# Copyright (c) 2001-2021 see AUTHORS.md file
#
# Permission is hereby granted, free of charge, to any person obtaining a copy
# of this software and associated documentation files (the 'Software'), to deal
# in the Software without restriction, including without limitation the rights
# to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
# copies of the Software, and to permit persons to whom the Software is
# furnished to do so, subject to the following conditions:
#
# The above copyright notice and this permission notice shall be included in
# all copies or substantial portions of the Software.
#
# THE SOFTWARE IS PROVIDED 'AS IS', WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
# IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
# FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
# AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
# LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
# OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
# THE SOFTWARE.
from benchmark import Benchmark


class Element:
    def __init__(self, value):
        self._value = value
        self.next = None

    def length(self):
        if self.next is None:
            return 1
        return 1 + self.next.length()


class List(Benchmark):
    def bench(self):
        result = self.tail(self.make_list(15), self.make_list(10), self.make_list(6))
        length = result.length()
        if length != 10:
            raise RuntimeError("List: expected 10, got " + str(length))

    def make_list(self, length):
        if length == 0:
            return None

        element = Element(length)
        element.next = self.make_list(length - 1)
        return element

    @staticmethod
    def is_shorter_than(first, second):
        first_tail = first
        second_tail = second

        while second_tail is not None:
            if first_tail is None:
                return True

            first_tail = first_tail.next
            second_tail = second_tail.next

        return False

    def tail(self, first, second, third):
        if self.is_shorter_than(second, first):  # pylint: disable=arguments-out-of-order
            return self.tail(
                self.tail(first.next, second, third),
                self.tail(second.next, third, first),
                self.tail(third.next, first, second),
            )
        return third
