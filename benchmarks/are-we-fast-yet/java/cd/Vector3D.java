/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/cd/Vector3D.java).
 * Changed for ns-next: names are written out, as in
 * benchmarks/are-we-fast-yet/CD.ns (x, y and z are coordinateX, coordinateY
 * and coordinateZ); the sum in dot, longer for the names, is split over two
 * lines. Upstream's Java file omits the licence notice, but it derives from
 * Purdue University's CDx, under the BSD-3 notice below, copied from the
 * header of benchmarks/are-we-fast-yet/CD.ns, which upstream's JavaScript and
 * SOM versions of CD carry as well.
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


public final class Vector3D {
  public final double coordinateX;
  public final double coordinateY;
  public final double coordinateZ;

  public Vector3D(final double coordinateX, final double coordinateY, final double coordinateZ) {
    this.coordinateX = coordinateX;
    this.coordinateY = coordinateY;
    this.coordinateZ = coordinateZ;
  }

  public Vector3D plus(final Vector3D other) {
    return new Vector3D(coordinateX + other.coordinateX,
                        coordinateY + other.coordinateY,
                        coordinateZ + other.coordinateZ);
  }

  public Vector3D minus(final Vector3D other) {
    return new Vector3D(coordinateX - other.coordinateX,
                        coordinateY - other.coordinateY,
                        coordinateZ - other.coordinateZ);
  }

  public double dot(final Vector3D other) {
    return coordinateX * other.coordinateX + coordinateY * other.coordinateY
        + coordinateZ * other.coordinateZ;
  }

  public double squaredMagnitude() {
    return this.dot(this);
  }

  public double magnitude() {
    return Math.sqrt(squaredMagnitude());
  }

  public Vector3D times(final double amount) {
    return new Vector3D(coordinateX * amount,
                        coordinateY * amount,
                        coordinateZ * amount);
  }
}
