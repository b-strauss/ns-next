/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/cd/Vector2D.java).
 * Changed for ns-next: names are written out, as in
 * benchmarks/are-we-fast-yet/newspeak/CD.ns (x and y are coordinateX and coordinateY).
 * Upstream's Java file omits the licence notice, but it derives from Purdue
 * University's CDx, under the BSD-3 notice below, copied from the header of
 * benchmarks/are-we-fast-yet/newspeak/CD.ns, which upstream's JavaScript and SOM
 * versions of CD carry as well.
 */
/*
 * Copyright (c) 2001-2010, Purdue University. All rights reserved.
 * Copyright (C) 2015 Apple Inc. All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions are met:
 *  * Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer.
 *  * Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in the
 *    documentation and/or other materials provided with the distribution.
 *  * Neither the name of the Purdue University nor the
 *    names of its contributors may be used to endorse or promote products
 *    derived from this software without specific prior written permission.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
 * ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
 * WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
 * DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER BE LIABLE FOR ANY
 * DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
 * (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
 * LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
 * ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
 * (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
 * SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 */
package cd;


public final class Vector2D implements Comparable<Vector2D> {
  public final double coordinateX;
  public final double coordinateY;

  public Vector2D(final double coordinateX, final double coordinateY) {
    this.coordinateX = coordinateX;
    this.coordinateY = coordinateY;
  }

  public Vector2D plus(final Vector2D other) {
    return new Vector2D(coordinateX + other.coordinateX,
                        coordinateY + other.coordinateY);
  }

  public Vector2D minus(final Vector2D other) {
    return new Vector2D(coordinateX - other.coordinateX,
                        coordinateY - other.coordinateY);
  }

  @Override
  public int compareTo(final Vector2D other) {
    int result = compareNumbers(this.coordinateX, other.coordinateX);
    if (result != 0) {
      return result;
    }
    return compareNumbers(this.coordinateY, other.coordinateY);
  }

  private static int compareNumbers(final double left, final double right) {
    if (left == right) {
      return 0;
    }
    if (left < right) {
      return -1;
    }
    if (left > right) {
      return 1;
    }

    // We say that NaN is smaller than non-NaN.
    if (left == left) {
      return 1;
    }
    return -1;
  }
}
