/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Java/src/Sieve.java).
 * Changed for ns-next: benchmark and verifyResult are bench, which runs once
 * and checks its own result, with the message of
 * benchmarks/are-we-fast-yet/Sieve.ns; the flags are filled by a loop, as in
 * that port, in place of java.util.Arrays.fill; the class is final; names are
 * written out.
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
public final class Sieve extends Benchmark {

  @Override
  public void bench() {
    boolean[] flags = new boolean[5000];
    for (int index = 0; index < flags.length; index++) {
      flags[index] = true;
    }
    int result = sieve(flags, 5000);
    if (result != 669) {
      throw new IllegalStateException("Sieve: expected 669, got " + result);
    }
  }

  int sieve(final boolean[] flags, final int size) {
    int primeCount = 0;

    for (int candidate = 2; candidate <= size; candidate++) {
      if (flags[candidate - 1]) {
        primeCount++;
        int multiple = candidate + candidate;
        while (multiple <= size) {
          flags[multiple - 1] = false;
          multiple += candidate;
        }
      }
    }
    return primeCount;
  }
}
