/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Java/src/Storage.java).
 * Changed for ns-next: benchmark and verifyResult are bench, which runs once
 * and checks its own result, with the message of
 * benchmarks/are-we-fast-yet/Storage.ns; the subtrees are built by a loop in
 * index order, as in that port, so the generator is drawn from in that order
 * (in place of java.util.Arrays.setAll); names are written out.
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
import som.Random;


public final class Storage extends Benchmark {

  private int count;

  @Override
  public void bench() {
    Random random = new Random();
    count = 0;
    buildTreeDepth(7, random);
    if (count != 5461) {
      throw new IllegalStateException("Storage: expected 5461, got " + count);
    }
  }

  private Object buildTreeDepth(final int depth, final Random random) {
    count++;
    if (depth == 1) {
      return new Object[random.next() % 10 + 1];
    } else {
      Object[] array = new Object[4];
      for (int index = 0; index < array.length; index++) {
        array[index] = buildTreeDepth(depth - 1, random);
      }
      return array;
    }
  }
}
