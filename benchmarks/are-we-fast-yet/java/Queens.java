/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Java/src/Queens.java).
 * Changed for ns-next: benchmark and verifyResult are bench, which runs once
 * and checks its own result, with the message of
 * benchmarks/are-we-fast-yet/newspeak/Queens.ns; the class is final; names are
 * written out; the arrays are filled in a loop, as the Newspeak port does,
 * because ns-next's core library has no native fill.
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
public final class Queens extends Benchmark {

  private boolean[] freeMaxs;
  private boolean[] freeRows;
  private boolean[] freeMins;
  private int[]     queenRows;

  @Override
  public void bench() {
    boolean result = true;
    for (int index = 0; index < 10; index++) {
      result = result && queens();
    }
    if (!result) {
      throw new IllegalStateException("Queens: expected true, got false");
    }
  }

  private boolean queens() {
    freeRows  = new boolean[ 8];
    for (int index = 0; index < 8; index++) {
      freeRows[index] = true;
    }
    freeMaxs  = new boolean[16];
    for (int index = 0; index < 16; index++) {
      freeMaxs[index] = true;
    }
    freeMins  = new boolean[16];
    for (int index = 0; index < 16; index++) {
      freeMins[index] = true;
    }
    queenRows = new     int[ 8];
    for (int index = 0; index < 8; index++) {
      queenRows[index] = -1;
    }

    return placeQueen(0);
  }

  private boolean placeQueen(final int column) {
    for (int row = 0; row < 8; row++) {
      if (getRowColumn(row, column)) {
        queenRows[row] = column;
        setRowColumn(row, column, false);

        if (column == 7) {
          return true;
        }

        if (placeQueen(column + 1)) {
          return true;
        }
        setRowColumn(row, column, true);
      }
    }
    return false;
  }

  private boolean getRowColumn(final int row, final int column) {
    return freeRows[row] && freeMaxs[column + row] && freeMins[column - row + 7];
  }

  private void setRowColumn(final int row, final int column, final boolean value) {
    freeRows[row             ] = value;
    freeMaxs[column + row    ] = value;
    freeMins[column - row + 7] = value;
  }
}
