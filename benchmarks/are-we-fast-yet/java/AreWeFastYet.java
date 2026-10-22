/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Java/src/Harness.java
 * and benchmarks/Java/src/Run.java), folded into one file.
 * Changed for ns-next: the command line, the measuring loop and the output
 * are those of benchmarks/are-we-fast-yet/AreWeFastYet.ns, so that both
 * harnesses run one protocol; the benchmarks are registered in that
 * harness's order; names are written out.
 *
 * java -cp <classes> AreWeFastYet
 *     [--timed [--warm-up <milliseconds>] [--measure <milliseconds>]] [Benchmark]:
 * without --timed, runs each benchmark (or the one named) once and prints
 * '<Name>: ok'; with it, measures each for at least --measure milliseconds
 * (20 by default) after --warm-up milliseconds (3 by default) of warm-up and
 * prints its runs per second with one fraction digit. An unknown name or a
 * malformed command line throws, so the JVM exits non-zero with the message
 * on stderr.
 *
 * The system property areWeFastYet.failFirstCheck=true replaces the first
 * selected benchmark by one whose bench always throws, so a test can see
 * that a failing benchmark prints no line; it is the harness's only test
 * hook.
 */
/* This code is based on the SOM class library.
 *
 * Copyright (c) 2001-2016 see AUTHORS.md file
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the 'Software'), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED 'AS IS', WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 */
import java.util.LinkedHashMap;
import java.util.Locale;
import java.util.Map;
import java.util.function.Supplier;

public final class AreWeFastYet {

  private static final String USAGE =
      "usage: java AreWeFastYet [--timed [--warm-up <milliseconds>] [--measure <milliseconds>]] [Benchmark]";

  private static final long DEFAULT_WARM_UP_MILLISECONDS = 3;
  private static final long DEFAULT_MEASURE_MILLISECONDS = 20;

  /** Every benchmark, in AreWeFastYet.ns's order. */
  private static final Map<String, Supplier<Benchmark>> BENCHMARKS =
      new LinkedHashMap<>();

  static {
    BENCHMARKS.put("Bounce", Bounce::new);
    BENCHMARKS.put("CD", CD::new);
    BENCHMARKS.put("DeltaBlue", DeltaBlue::new);
    BENCHMARKS.put("DeltaBlueTon80", DeltaBlueTon80::new);
    BENCHMARKS.put("Havlak", Havlak::new);
    BENCHMARKS.put("Json", Json::new);
    BENCHMARKS.put("List", List::new);
    BENCHMARKS.put("Mandelbrot", Mandelbrot::new);
    BENCHMARKS.put("NBody", NBody::new);
    BENCHMARKS.put("Permute", Permute::new);
    BENCHMARKS.put("Queens", Queens::new);
    BENCHMARKS.put("Richards", Richards::new);
    BENCHMARKS.put("RichardsTon80", RichardsTon80::new);
    BENCHMARKS.put("Sieve", Sieve::new);
    BENCHMARKS.put("Storage", Storage::new);
    BENCHMARKS.put("Towers", Towers::new);
  }

  private AreWeFastYet() { }

  public static void main(final String[] arguments) {
    boolean timed = arguments.length >= 1 && arguments[0].equals("--timed");
    long warmUpMilliseconds = -1;
    long measureMilliseconds = -1;
    int index = timed ? 1 : 0;
    while (timed && index < arguments.length
        && (arguments[index].equals("--warm-up") || arguments[index].equals("--measure"))) {
      if (index + 1 >= arguments.length) {
        throw new IllegalArgumentException(USAGE);
      }
      long milliseconds = milliseconds(arguments[index + 1]);
      if (arguments[index].equals("--warm-up")) {
        if (warmUpMilliseconds >= 0) {
          throw new IllegalArgumentException(USAGE);
        }
        warmUpMilliseconds = milliseconds;
      } else {
        if (measureMilliseconds >= 0) {
          throw new IllegalArgumentException(USAGE);
        }
        measureMilliseconds = milliseconds;
      }
      index += 2;
    }
    if (arguments.length > index + 1) {
      throw new IllegalArgumentException(USAGE);
    }
    String name = arguments.length == index + 1 ? arguments[index] : null;
    run(name, timed,
        (warmUpMilliseconds >= 0 ? warmUpMilliseconds : DEFAULT_WARM_UP_MILLISECONDS) * 1_000_000L,
        (measureMilliseconds >= 0 ? measureMilliseconds : DEFAULT_MEASURE_MILLISECONDS) * 1_000_000L);
  }

  /**
   * A flag's value: a non-negative decimal integer, or the usage. The
   * text is what Newspeak's Integer parse: reads, an optional '-' and ASCII
   * digits, so a '+' or another script's digits, which Long.parseLong would
   * take, are refused on both sides. A value whose nanoseconds a long cannot
   * hold, some 292 years, is refused too, where Newspeak's unbounded
   * integers would take it.
   */
  private static long milliseconds(final String value) {
    if (!value.matches("-?[0-9]+")) {
      throw new IllegalArgumentException(USAGE);
    }
    long milliseconds;
    try {
      milliseconds = Long.parseLong(value);
    } catch (NumberFormatException exception) {
      throw new IllegalArgumentException(USAGE, exception);
    }
    if (milliseconds < 0 || milliseconds > Long.MAX_VALUE / 1_000_000L) {
      throw new IllegalArgumentException(USAGE);
    }
    return milliseconds;
  }

  private static void run(final String name, final boolean timed,
      final long warmUpNanoseconds, final long measureNanoseconds) {
    boolean found = false;
    boolean failNext = Boolean.getBoolean("areWeFastYet.failFirstCheck");
    for (Map.Entry<String, Supplier<Benchmark>> entry : BENCHMARKS.entrySet()) {
      if (name == null || entry.getKey().equals(name)) {
        found = true;
        Benchmark benchmark = failNext ? new FailingBenchmark() : entry.getValue().get();
        failNext = false;
        report(entry.getKey(), benchmark, timed, warmUpNanoseconds, measureNanoseconds);
      }
    }
    if (!found) {
      throw new IllegalArgumentException("Unknown benchmark: " + name);
    }
  }

  /**
   * The line is written only once the benchmark has run, so a benchmark
   * that throws leaves no line on stdout.
   */
  private static void report(final String name, final Benchmark benchmark,
      final boolean timed, final long warmUpNanoseconds, final long measureNanoseconds) {
    String result;
    if (timed) {
      measure(benchmark, warmUpNanoseconds);
      double score = measure(benchmark, measureNanoseconds);
      result = String.format(Locale.ROOT, "%.1f", score);
    } else {
      benchmark.bench();
      result = "ok";
    }
    System.out.println(name + ": " + result);
  }

  /**
   * Primordial Soup's measure: runs the benchmark until the time is spent
   * and answers runs per second.
   */
  static double measure(final Benchmark benchmark, final long nanoseconds) {
    long runs = 0;
    long start = System.nanoTime();
    long elapsed;
    do {
      benchmark.bench();
      runs++;
      elapsed = System.nanoTime() - start;
    } while (elapsed < nanoseconds);
    return (runs * 1_000_000_000.0) / elapsed;
  }

  /**
   * Stands in for the first selected benchmark under
   * areWeFastYet.failFirstCheck: a benchmark whose check always fails.
   */
  private static final class FailingBenchmark extends Benchmark {
    @Override
    public void bench() {
      throw new IllegalStateException("forced failure");
    }
  }
}
