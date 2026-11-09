# Adapted for ns-next from Are We Fast Yet
# (https://github.com/smarr/are-we-fast-yet, commit
# 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Python/queens.py).
# Changed for ns-next: benchmark and verify_result are bench, which runs once
# and checks its own result, raising a RuntimeError with the message of
# benchmarks/are-we-fast-yet/Queens.ns; names are written out.

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


class Queens(Benchmark):
    def __init__(self):
        self._free_maxs = None
        self._free_rows = None
        self._free_mins = None
        self._queen_rows = None

    def bench(self):
        result = True
        for _ in range(10):
            result = result and self.queens()
        if not result:
            raise RuntimeError("Queens: expected true, got false")

    def queens(self):
        self._free_rows = [True] * 8
        self._free_maxs = [True] * 16
        self._free_mins = [True] * 16
        self._queen_rows = [-1] * 8

        return self.place_queen(0)

    def place_queen(self, column):
        for row in range(8):
            if self.get_row_column(row, column):
                self._queen_rows[row] = column
                self.set_row_column(row, column, False)

                if column == 7:
                    return True
                if self.place_queen(column + 1):
                    return True

                self.set_row_column(row, column, True)
        return False

    def get_row_column(self, row, column):
        return (
            self._free_rows[row]
            and self._free_maxs[column + row]
            and self._free_mins[column - row + 7]
        )

    def set_row_column(self, row, column, value):
        self._free_rows[row] = value
        self._free_maxs[column + row] = value
        self._free_mins[column - row + 7] = value
