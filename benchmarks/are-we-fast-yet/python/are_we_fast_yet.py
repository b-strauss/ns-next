# Adapted for ns-next from Are We Fast Yet
# (https://github.com/smarr/are-we-fast-yet, commit
# 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Python/harness.py and
# benchmarks/Python/run.py), folded into one file.
# Changed for ns-next: the command line, the measuring loop and the output
# are those of benchmarks/are-we-fast-yet/newspeak/AreWeFastYet.ns and
# java/AreWeFastYet.java, so that the three harnesses run one protocol; the
# benchmarks are registered in that harness's order; names are written out.
#
# python are_we_fast_yet.py
#     [--timed [--warm-up <milliseconds>] [--measure <milliseconds>]] [Benchmark]:
# without --timed, runs each benchmark (or the one named) once and prints
# '<Name>: ok'; with it, measures each for at least --measure milliseconds
# (20 by default) after --warm-up milliseconds (3 by default) of warm-up and
# prints its runs per second with one fraction digit. An unknown name or a
# malformed command line ends the run with exit status 1 and the message on
# stderr; a benchmark whose check fails raises, so Python ends the run with
# exit status 1 and the traceback on stderr.
#
# The environment variable ARE_WE_FAST_YET_FAIL_FIRST_CHECK=true replaces the
# first selected benchmark by one whose bench always raises, so a test can see
# that a failing benchmark prints no line; it is the harness's only test hook.

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
import importlib
import os
import re
import sys
from time import perf_counter_ns

from benchmark import Benchmark

USAGE = (
    "usage: python are_we_fast_yet.py "
    "[--timed [--warm-up <milliseconds>] [--measure <milliseconds>]] [Benchmark]"
)

DEFAULT_WARM_UP_MILLISECONDS = 3
DEFAULT_MEASURE_MILLISECONDS = 20

# The largest value a flag may take: the Java harness's bound, the
# milliseconds whose nanoseconds a long can hold, kept so that both harnesses
# refuse the same command lines.
MAXIMUM_MILLISECONDS = (2**63 - 1) // 1_000_000

# Every benchmark, in AreWeFastYet.ns's order; each is the class of its name
# in the module of its name in lower case, imported only when selected.
BENCHMARKS = [
    "Bounce",
    "CD",
    "DeltaBlue",
    "DeltaBlueTon80",
    "Havlak",
    "Json",
    "List",
    "Mandelbrot",
    "NBody",
    "Permute",
    "Queens",
    "Richards",
    "RichardsTon80",
    "Sieve",
    "Storage",
    "Towers",
]


class UsageError(Exception):
    """A command line the harness refuses; its message goes to stderr."""


def main(arguments):
    timed = len(arguments) >= 1 and arguments[0] == "--timed"
    warm_up_milliseconds = -1
    measure_milliseconds = -1
    index = 1 if timed else 0
    while (
        timed
        and index < len(arguments)
        and arguments[index] in ("--warm-up", "--measure")
    ):
        if index + 1 >= len(arguments):
            raise UsageError(USAGE)
        value = milliseconds(arguments[index + 1])
        if arguments[index] == "--warm-up":
            if warm_up_milliseconds >= 0:
                raise UsageError(USAGE)
            warm_up_milliseconds = value
        else:
            if measure_milliseconds >= 0:
                raise UsageError(USAGE)
            measure_milliseconds = value
        index += 2
    if len(arguments) > index + 1:
        raise UsageError(USAGE)
    name = arguments[index] if len(arguments) == index + 1 else None
    if warm_up_milliseconds < 0:
        warm_up_milliseconds = DEFAULT_WARM_UP_MILLISECONDS
    if measure_milliseconds < 0:
        measure_milliseconds = DEFAULT_MEASURE_MILLISECONDS
    run(name, timed, warm_up_milliseconds * 1_000_000, measure_milliseconds * 1_000_000)


def milliseconds(value):
    """A flag's value: a non-negative decimal integer, or the usage. The text
    is what Newspeak's Integer parse: reads, an optional '-' and ASCII digits,
    so a '+', surrounding blanks or another script's digits, which int would
    take, are refused on every side."""
    if not re.fullmatch("-?[0-9]+", value, re.ASCII):
        raise UsageError(USAGE)
    result = int(value)
    if result < 0 or result > MAXIMUM_MILLISECONDS:
        raise UsageError(USAGE)
    return result


def run(name, timed, warm_up_nanoseconds, measure_nanoseconds):
    found = False
    fail_next = (
        os.environ.get("ARE_WE_FAST_YET_FAIL_FIRST_CHECK", "").lower() == "true"
    )
    for benchmark_name in BENCHMARKS:
        if name is None or benchmark_name == name:
            found = True
            benchmark = FailingBenchmark() if fail_next else instantiate(benchmark_name)
            fail_next = False
            report(benchmark_name, benchmark, timed, warm_up_nanoseconds, measure_nanoseconds)
    if not found:
        raise UsageError("Unknown benchmark: " + str(name))


def instantiate(name):
    module = importlib.import_module(name.lower())
    return getattr(module, name)()


def report(name, benchmark, timed, warm_up_nanoseconds, measure_nanoseconds):
    """The line is written only once the benchmark has run, so a benchmark
    that raises leaves no line on stdout."""
    if timed:
        measure(benchmark, warm_up_nanoseconds)
        score = measure(benchmark, measure_nanoseconds)
        result = "%.1f" % score
    else:
        benchmark.bench()
        result = "ok"
    print(name + ": " + result, flush=True)


def measure(benchmark, nanoseconds):
    """Primordial Soup's measure: runs the benchmark until the time is spent
    and answers runs per second."""
    runs = 0
    start = perf_counter_ns()
    while True:
        benchmark.bench()
        runs += 1
        elapsed = perf_counter_ns() - start
        if elapsed >= nanoseconds:
            break
    return (runs * 1_000_000_000.0) / elapsed


class FailingBenchmark(Benchmark):
    """Stands in for the first selected benchmark under
    ARE_WE_FAST_YET_FAIL_FIRST_CHECK: a benchmark whose check always fails."""

    def bench(self):
        raise RuntimeError("forced failure")


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except UsageError as error:
        print(error, file=sys.stderr)
        sys.exit(1)
