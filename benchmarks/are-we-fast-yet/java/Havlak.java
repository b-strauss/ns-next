/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/Havlak.java).
 * Changed for ns-next: innerBenchmarkLoop, benchmark and verifyResult are
 * bench, which runs once at the size of benchmarks/are-we-fast-yet/Havlak.ns,
 * 1500 dummy loops, 50 loop-finding iterations and 10, 10 and 5 nested loops,
 * as upstream runs it at that size, and checks 6102 loops in 5213 nodes with
 * that port's message; the checks for upstream's other sizes and the printing
 * of an unknown size's result are dropped; names are written out. That port's
 * other departures answer Primordial Soup's protocol and the 1-based
 * numbering of its collections, and have no counterpart here.
 */
/*
 * Copyright (c) 2001-2016 Stefan Marr
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
import havlak.LoopTesterApp;

public final class Havlak extends Benchmark {

  @Override
  public void bench() {
    int[] result = (new LoopTesterApp()).main(
        1500, 50, 10 /* was 100 */, 10, 5);
    if (result[0] != 6102 || result[1] != 5213) {
      throw new IllegalStateException(
          "Havlak: expected 6102 loops and 5213 nodes, got "
          + result[0] + " and " + result[1]);
    }
  }
}
