# Adapted for ns-next from Are We Fast Yet
# (https://github.com/smarr/are-we-fast-yet, commit
# 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Python/permute.py).
# Changed for ns-next: benchmark and verify_result are bench, which runs once
# and checks its own result, raising a RuntimeError with the message of
# benchmarks/are-we-fast-yet/newspeak/Permute.ns; names are written out.

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


class Permute(Benchmark):
    def __init__(self):
        self._count = 0
        self._values = None

    def bench(self):
        self._count = 0
        self._values = [0] * 6
        self._permute(6)

        if self._count != 8660:
            raise RuntimeError("Permute: expected 8660, got " + str(self._count))

    def _permute(self, size):
        self._count += 1
        if size != 0:
            last_index = size - 1
            self._permute(last_index)
            for index in range(last_index, -1, -1):
                self._swap(last_index, index)
                self._permute(last_index)
                self._swap(last_index, index)

    def _swap(self, first, second):
        temp = self._values[first]
        self._values[first] = self._values[second]
        self._values[second] = temp
