/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/cd/Motion.java).
 * Changed for ns-next: names are written out, as in
 * benchmarks/are-we-fast-yet/CD.ns. Upstream's Java file omits the licence
 * notice, but it derives from Purdue University's CDx, under the BSD-3 notice
 * below, copied from the header of benchmarks/are-we-fast-yet/CD.ns, which
 * upstream's JavaScript and SOM versions of CD carry as well.
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


public class Motion {
  public final CallSign callsign;
  public final Vector3D positionOne;
  public final Vector3D positionTwo;

  public Motion(final CallSign callsign, final Vector3D positionOne,
      final Vector3D positionTwo) {
    this.callsign = callsign;
    this.positionOne = positionOne;
    this.positionTwo = positionTwo;
  }

  private Vector3D delta() {
    return positionTwo.minus(this.positionOne);
  }

  public Vector3D findIntersection(final Motion other) {
    Vector3D initialOne = this.positionOne;
    Vector3D initialTwo = other.positionOne;
    Vector3D vectorOne = delta();
    Vector3D vectorTwo = other.delta();
    final double radius = Constants.PROXIMITY_RADIUS;

    // this test is not geometrical 3-d intersection test, it takes the fact that the aircraft move
    // into account ; so it is more like a 4d test
    // (it assumes that both of the aircraft have a constant speed over the tested interval)

    // we thus have two points, each of them moving on its line segment at constant speed ; we are looking
    // for times when the distance between these two points is smaller than r

    // vec1 is vector of aircraft 1
    // vec2 is vector of aircraft 2

    // a = (V2 - V1)^T * (V2 - V1)
    double squaredSpeed = vectorTwo.minus(vectorOne).squaredMagnitude();

    if (squaredSpeed != 0.0) {
      // we are first looking for instances of time when the planes are exactly r from each other
      // at least one plane is moving ; if the planes are moving in parallel, they do not have constant speed

      // if the planes are moving in parallel, then
      //   if the faster starts behind the slower, we can have 2, 1, or 0 solutions
      //   if the faster plane starts in front of the slower, we can have 0 or 1 solutions

      // if the planes are not moving in parallel, then

      // point P1 = I1 + vV1
      // point P2 = I2 + vV2
      //   - looking for v, such that dist(P1,P2) = || P1 - P2 || = r

      // it follows that || P1 - P2 || = sqrt( < P1-P2, P1-P2 > )
      //   0 = -r^2 + < P1 - P2, P1 - P2 >
      //  from properties of dot product
      //   0 = -r^2 + <I1-I2,I1-I2> + v * 2<I1-I2, V1-V2> + v^2 *<V1-V2,V1-V2>
      //   so we calculate a, b, c - and solve the quadratic equation
      //   0 = c + bv + av^2

      // b = 2 * <I1-I2, V1-V2>
      double linear = 2.0 * initialOne.minus(initialTwo).dot(vectorOne.minus(vectorTwo));

      // c = -r^2 + (I2 - I1)^T * (I2 - I1)
      double constant = -radius * radius + initialTwo.minus(initialOne).squaredMagnitude();

      double discriminant = linear * linear - 4.0 * squaredSpeed * constant;
      if (discriminant < 0.0) {
        return null;
      }

      double timeOne = (-linear - Math.sqrt(discriminant)) / (2.0 * squaredSpeed);
      double timeTwo = (-linear + Math.sqrt(discriminant)) / (2.0 * squaredSpeed);

      if (timeOne <= timeTwo && ((timeOne <= 1.0 && 1.0 <= timeTwo) ||
                                 (timeOne <= 0.0 && 0.0 <= timeTwo) ||
                                 (0.0 <= timeOne && timeTwo <= 1.0))) {
        // Pick a good "time" at which to report the collision.
        double time;
        if (timeOne <= 0.0) {
          // The collision started before this frame. Report it at the start of the frame.
          time = 0.0;
        } else {
          // The collision started during this frame. Report it at that moment.
          time = timeOne;
        }

        Vector3D resultOne = initialOne.plus(vectorOne.times(time));
        Vector3D resultTwo = initialTwo.plus(vectorTwo.times(time));

        Vector3D result = resultOne.plus(resultTwo).times(0.5);
        if (result.coordinateX >= Constants.MIN_X &&
            result.coordinateX <= Constants.MAX_X &&
            result.coordinateY >= Constants.MIN_Y &&
            result.coordinateY <= Constants.MAX_Y &&
            result.coordinateZ >= Constants.MIN_Z &&
            result.coordinateZ <= Constants.MAX_Z) {
          return result;
        }
      }

      return null;
    }

    // the planes have the same speeds and are moving in parallel (or they are not moving at all)
    // they  thus have the same distance all the time ; we calculate it from the initial point

    // dist = || i2 - i1 || = sqrt(  ( i2 - i1 )^T * ( i2 - i1 ) )
    double distance = initialTwo.minus(initialOne).magnitude();
    if (distance <= radius) {
      return initialOne.plus(initialTwo).times(0.5);
    }

    return null;
  }
}
