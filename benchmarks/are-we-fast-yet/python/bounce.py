# Adapted for ns-next from Are We Fast Yet
# (https://github.com/smarr/are-we-fast-yet, commit
# 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Python/bounce.py).
# Changed for ns-next: benchmark and verify_result are bench, which runs once
# and checks its own result, raising a RuntimeError with the message of
# benchmarks/are-we-fast-yet/Bounce.ns; names are written out.

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
from som.random import Random


class Ball:
    def __init__(self, random):
        self._horizontal = random.next() % 500
        self._vertical = random.next() % 500
        self._horizontal_velocity = (random.next() % 300) - 150
        self._vertical_velocity = (random.next() % 300) - 150

    def bounce(self):
        horizontal_limit = 500
        vertical_limit = 500
        bounced = False

        self._horizontal += self._horizontal_velocity
        self._vertical += self._vertical_velocity

        if self._horizontal > horizontal_limit:
            self._horizontal = horizontal_limit
            self._horizontal_velocity = -abs(self._horizontal_velocity)
            bounced = True

        if self._horizontal < 0:
            self._horizontal = 0
            self._horizontal_velocity = abs(self._horizontal_velocity)
            bounced = True

        if self._vertical > vertical_limit:
            self._vertical = vertical_limit
            self._vertical_velocity = -abs(self._vertical_velocity)
            bounced = True

        if self._vertical < 0:
            self._vertical = 0
            self._vertical_velocity = abs(self._vertical_velocity)
            bounced = True

        return bounced


class Bounce(Benchmark):
    def bench(self):
        random = Random()

        ball_count = 100
        bounces = 0
        balls = [None] * ball_count

        for index in range(ball_count):
            balls[index] = Ball(random)

        for _ in range(50):
            for ball in balls:
                if ball.bounce():
                    bounces += 1

        if bounces != 1331:
            raise RuntimeError("Bounce: expected 1331, got " + str(bounces))
