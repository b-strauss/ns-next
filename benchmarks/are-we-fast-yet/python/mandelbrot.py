# Adapted for ns-next from Are We Fast Yet
# (https://github.com/smarr/are-we-fast-yet, commit
# 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Python/mandelbrot.py).
# Changed for ns-next: inner_benchmark_loop, benchmark and verify_result are
# bench, which runs once at benchmarks/are-we-fast-yet/newspeak/Mandelbrot.ns's size,
# 750, and checks its result, 50, raising a RuntimeError with that port's
# message; the checks for upstream's other sizes, 500 and 1, and the printing
# of an unknown size's result are dropped; names are written out.

# This benchmark has been modified based on the SOM benchmark.
#
# Copyright (C) 2004-2013 Brent Fulgham
#
# All rights reserved.
#
# Redistribution and use in source and binary forms, with or without
# modification, are permitted provided that the following conditions are met:
#
#   * Redistributions of source code must retain the above copyright notice,
#     this list of conditions and the following disclaimer.
#
#   * Redistributions in binary form must reproduce the above copyright notice,
#     this list of conditions and the following disclaimer in the documentation
#     and/or other materials provided with the distribution.
#
#   * Neither the name of "The Computer Language Benchmarks Game" nor the name
#     of "The Computer Language Shootout Benchmarks" nor the names of its
#     contributors may be used to endorse or promote products derived from this
#     software without specific prior written permission.
#
# THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
# AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
# IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
# DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE
# FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
# DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
# SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
# CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
# OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
# OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

# The Computer Language Benchmarks Game
# http://benchmarksgame.alioth.debian.org
#
#  contributed by Karl von Laudermann
#  modified by Jeremy Echols
#  modified by Detlef Reichl
#  modified by Joseph LaFata
#  modified by Peter Zotov

# http://benchmarksgame.alioth.debian.org/u64q/program.php?test=mandelbrot&lang=yarv&id=3
from benchmark import Benchmark


class Mandelbrot(Benchmark):
    def bench(self):
        result = self._mandelbrot(750)
        if result != 50:
            raise RuntimeError("Mandelbrot: expected 50, got " + str(result))

    @staticmethod
    def _mandelbrot(size):
        _sum = 0
        byte_accumulator = 0
        bit_number = 0

        row = 0

        while row < size:
            imaginary_constant = (2.0 * row / size) - 1.0
            column = 0

            while column < size:
                real_squared = 0.0
                imaginary = 0.0
                imaginary_squared = 0.0
                real_constant = (2.0 * column / size) - 1.5

                iteration = 0
                not_done = True
                escape = 0
                while not_done and iteration < 50:
                    real = real_squared - imaginary_squared + real_constant
                    imaginary = 2.0 * real * imaginary + imaginary_constant

                    real_squared = real * real
                    imaginary_squared = imaginary * imaginary

                    if real_squared + imaginary_squared > 4.0:
                        not_done = False
                        escape = 1
                    iteration += 1

                byte_accumulator = (byte_accumulator << 1) + escape
                bit_number = bit_number + 1

                if bit_number == 8:
                    _sum ^= byte_accumulator
                    byte_accumulator = 0
                    bit_number = 0
                elif column == size - 1:
                    byte_accumulator <<= 8 - bit_number
                    _sum ^= byte_accumulator
                    byte_accumulator = 0
                    bit_number = 0
                column += 1
            row += 1

        return _sum
