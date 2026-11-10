/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Java/src/Permute.java).
 * Changed for ns-next: benchmark and verifyResult are bench, which runs once
 * and checks its own result, with the message of
 * benchmarks/are-we-fast-yet/newspeak/Permute.ns; names are written out.
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
public final class Permute extends Benchmark {
  private int count;
  private int[] values;

  @Override
  public void bench() {
    count  = 0;
    values = new int[6];
    permute(6);
    if (count != 8660) {
      throw new IllegalStateException("Permute: expected 8660, got " + count);
    }
  }

  void permute(final int size) {
    count++;
    if (size != 0) {
      int lastIndex = size - 1;
      permute(lastIndex);
      for (int index = lastIndex; index >= 0; index--) {
        swap(lastIndex, index);
        permute(lastIndex);
        swap(lastIndex, index);
      }
    }
  }

  private void swap(final int first, final int second) {
    int temp = values[first];
    values[first] = values[second];
    values[second] = temp;
  }
}
