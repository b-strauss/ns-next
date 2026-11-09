# Adapted for ns-next from Are We Fast Yet
# (https://github.com/smarr/are-we-fast-yet, commit
# 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Python/nbody.py).
# Changed for ns-next: inner_benchmark_loop, benchmark and verify_result are
# bench, which runs once at benchmarks/are-we-fast-yet/NBody.ns's size, 250000
# steps, and checks its energy, -0.1690859889909308, raising a RuntimeError
# with that port's message; the check for upstream's other size, 1, and the
# printing of an unknown size's result are dropped; names are written out,
# DAYS_PER_YER's misspelling corrected. Upstream's file carries no licence
# notice; its comment follows as it is.

# The Computer Language Benchmarks Game
# http://shootout.alioth.debian.org/
#
# Based on nbody.java and adapted based on the SOM version.
from math import sqrt

from benchmark import Benchmark

PI_VALUE = 3.141592653589793
SOLAR_MASS = 4 * PI_VALUE * PI_VALUE
DAYS_PER_YEAR = 365.24


class _Body:
    def __init__(self, position_x, position_y, position_z, velocity_x, velocity_y, velocity_z, mass):
        self.position_x = position_x
        self.position_y = position_y
        self.position_z = position_z
        self.velocity_x = velocity_x * DAYS_PER_YEAR
        self.velocity_y = velocity_y * DAYS_PER_YEAR
        self.velocity_z = velocity_z * DAYS_PER_YEAR
        self.mass = mass * SOLAR_MASS

    def offset_momentum(self, momentum_x, momentum_y, momentum_z):
        self.velocity_x = -(momentum_x / SOLAR_MASS)
        self.velocity_y = -(momentum_y / SOLAR_MASS)
        self.velocity_z = -(momentum_z / SOLAR_MASS)


def jupiter():
    return _Body(
        4.84143144246472090e00,
        -1.16032004402742839e00,
        -1.03622044471123109e-01,
        1.66007664274403694e-03,
        7.69901118419740425e-03,
        -6.90460016972063023e-05,
        9.54791938424326609e-04,
    )


def saturn():
    return _Body(
        8.34336671824457987e00,
        4.12479856412430479e00,
        -4.03523417114321381e-01,
        -2.76742510726862411e-03,
        4.99852801234917238e-03,
        2.30417297573763929e-05,
        2.85885980666130812e-04,
    )


def uranus():
    return _Body(
        1.28943695621391310e01,
        -1.51111514016986312e01,
        -2.23307578892655734e-01,
        2.96460137564761618e-03,
        2.37847173959480950e-03,
        -2.96589568540237556e-05,
        4.36624404335156298e-05,
    )


def neptune():
    return _Body(
        1.53796971148509165e01,
        -2.59193146099879641e01,
        1.79258772950371181e-01,
        2.68067772490389322e-03,
        1.62824170038242295e-03,
        -9.51592254519715870e-05,
        5.15138902046611451e-05,
    )


def sun():
    return _Body(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0)


class NBodySystem:
    def __init__(self):
        self._bodies = self._create_bodies()

    @staticmethod
    def _create_bodies():
        bodies = [sun(), jupiter(), saturn(), uranus(), neptune()]

        momentum_x = 0.0
        momentum_y = 0.0
        momentum_z = 0.0

        for body in bodies:
            momentum_x += body.velocity_x * body.mass
            momentum_y += body.velocity_y * body.mass
            momentum_z += body.velocity_z * body.mass

        bodies[0].offset_momentum(momentum_x, momentum_y, momentum_z)
        return bodies

    def advance(self, time_step):
        for first in range(len(self._bodies)):
            first_body = self._bodies[first]

            for second in range(first + 1, len(self._bodies)):
                second_body = self._bodies[second]

                delta_x = first_body.position_x - second_body.position_x
                delta_y = first_body.position_y - second_body.position_y
                delta_z = first_body.position_z - second_body.position_z

                distance_squared = delta_x * delta_x + delta_y * delta_y + delta_z * delta_z
                distance = sqrt(distance_squared)
                magnitude = time_step / (distance_squared * distance)

                first_body.velocity_x = first_body.velocity_x - (delta_x * second_body.mass * magnitude)
                first_body.velocity_y = first_body.velocity_y - (delta_y * second_body.mass * magnitude)
                first_body.velocity_z = first_body.velocity_z - (delta_z * second_body.mass * magnitude)

                second_body.velocity_x = second_body.velocity_x + (delta_x * first_body.mass * magnitude)
                second_body.velocity_y = second_body.velocity_y + (delta_y * first_body.mass * magnitude)
                second_body.velocity_z = second_body.velocity_z + (delta_z * first_body.mass * magnitude)

        for body in self._bodies:
            body.position_x = body.position_x + time_step * body.velocity_x
            body.position_y = body.position_y + time_step * body.velocity_y
            body.position_z = body.position_z + time_step * body.velocity_z

    def energy(self):
        energy = 0.0

        for first in range(len(self._bodies)):
            first_body = self._bodies[first]
            energy += (
                0.5
                * first_body.mass
                * (
                    first_body.velocity_x * first_body.velocity_x
                    + first_body.velocity_y * first_body.velocity_y
                    + first_body.velocity_z * first_body.velocity_z
                )
            )

            for second in range(first + 1, len(self._bodies)):
                second_body = self._bodies[second]
                delta_x = first_body.position_x - second_body.position_x
                delta_y = first_body.position_y - second_body.position_y
                delta_z = first_body.position_z - second_body.position_z

                distance = sqrt(delta_x * delta_x + delta_y * delta_y + delta_z * delta_z)
                energy -= (first_body.mass * second_body.mass) / distance
        return energy


class NBody(Benchmark):
    def bench(self):
        system = NBodySystem()
        for _ in range(250000):
            system.advance(0.01)

        energy = system.energy()
        if energy != -0.1690859889909308:
            raise RuntimeError(
                "NBody: expected -0.1690859889909308, got " + str(energy)
            )
