/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/cd/CollisionDetector.java).
 * Changed for ns-next: names are written out, as in
 * benchmarks/are-we-fast-yet/CD.ns; the two tests for no motion in isInVoxel,
 * longer for the names, are each split over two lines. Upstream's Java file
 * omits the licence notice, but it derives from Purdue University's CDx, under
 * the BSD-3 notice below, copied from the header of
 * benchmarks/are-we-fast-yet/CD.ns, which upstream's JavaScript and SOM
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

import som.Vector;

public final class CollisionDetector {
  private final RedBlackTree<CallSign, Vector3D> state;

  public CollisionDetector() {
    state = new RedBlackTree<>();
  }

  public Vector<Collision> handleNewFrame(final Vector<Aircraft> frame) {
    Vector<Motion> motions = new Vector<>();
    RedBlackTree<CallSign, Boolean> seen = new RedBlackTree<>();

    frame.forEach(aircraft -> {
      Vector3D oldPosition = state.put(aircraft.callsign, aircraft.position);
      Vector3D newPosition = aircraft.position;
      seen.put(aircraft.callsign, true);

      if (oldPosition == null) {
        // Treat newly introduced aircraft as if they were stationary.
        oldPosition = newPosition;
      }

      motions.append(new Motion(aircraft.callsign, oldPosition, newPosition));
    });

    // Remove aircraft that are no longer present.
    Vector<CallSign> toRemove = new Vector<>();
    state.forEach(entry -> {
      if (!seen.get(entry.key)) {
        toRemove.append(entry.key);
      }
    });

    toRemove.forEach(callsign -> state.remove(callsign));

    Vector<Vector<Motion>> allReduced = reduceCollisionSet(motions);
    Vector<Collision> collisions = new Vector<>();
    allReduced.forEach(reduced -> {
      for (int first = 0; first < reduced.size(); ++first) {
        Motion motionOne = reduced.at(first);
        for (int second = first + 1; second < reduced.size(); ++second) {
          Motion motionTwo = reduced.at(second);
          Vector3D collision = motionOne.findIntersection(motionTwo);
          if (collision != null) {
            collisions.append(new Collision(motionOne.callsign, motionTwo.callsign, collision));
          }
        }
      }
    });

    return collisions;
  }

  private static boolean isInVoxel(final Vector2D voxel, final Motion motion) {
    if (voxel.coordinateX > Constants.MAX_X ||
        voxel.coordinateX < Constants.MIN_X ||
        voxel.coordinateY > Constants.MAX_Y ||
        voxel.coordinateY < Constants.MIN_Y) {
      return false;
    }

    Vector3D start  = motion.positionOne;
    Vector3D finish = motion.positionTwo;

    double voxelSize = Constants.GOOD_VOXEL_SIZE;
    double radius    = Constants.PROXIMITY_RADIUS / 2.0;

    double voxelX = voxel.coordinateX;
    double startX = start.coordinateX;
    double deltaX = finish.coordinateX - start.coordinateX;

    double voxelY = voxel.coordinateY;
    double startY = start.coordinateY;
    double deltaY = finish.coordinateY - start.coordinateY;

    double lowX = (voxelX - radius - startX) / deltaX;
    double highX = (voxelX + voxelSize + radius - startX) / deltaX;

    if (deltaX < 0.0) {
      double temp = lowX;
      lowX = highX;
      highX = temp;
    }

    double lowY  = (voxelY - radius - startY) / deltaY;
    double highY = (voxelY + voxelSize + radius - startY) / deltaY;

    if (deltaY < 0.0) {
      double temp = lowY;
      lowY = highY;
      highY = temp;
    }

    return (((deltaX == 0.0 && voxelX <= startX + radius
              && startX - radius <= voxelX + voxelSize) /* no motion in x */ ||
             (lowX <= 1.0 && 1.0 <= highX) || (lowX <= 0.0 && 0.0 <= highX) ||
              (0.0 <= lowX && highX <= 1.0)) &&
            ((deltaY == 0.0 && voxelY <= startY + radius
              && startY - radius <= voxelY + voxelSize) /* no motion in y */ ||
             ((lowY <= 1.0 && 1.0 <= highY) || (lowY <= 0.0 && 0.0 <= highY) ||
              (0.0 <= lowY && highY <= 1.0))) &&
            (deltaX == 0.0 || deltaY == 0.0 || /* no motion in x or y or both */
             (lowY <= highX && highX <= highY) ||
             (lowY <= lowX && lowX <= highY) ||
             (lowX <= lowY && highY <= highX)));
  }

  private static final Vector2D horizontal = new Vector2D(Constants.GOOD_VOXEL_SIZE, 0.0);
  private static final Vector2D vertical   = new Vector2D(0.0, Constants.GOOD_VOXEL_SIZE);

  private static void putIntoMap(
      final RedBlackTree<Vector2D, Vector<Motion>> voxelMap,
      final Vector2D voxel, final Motion motion) {
    Vector<Motion> array = voxelMap.get(voxel);
    if (array == null) {
      array = new Vector<>();
      voxelMap.put(voxel, array);
    }
    array.append(motion);
  }

  private static void recurse(
      final RedBlackTree<Vector2D, Vector<Motion>> voxelMap,
      final RedBlackTree<Vector2D, Boolean> seen,
      final Vector2D nextVoxel, final Motion motion) {
    if (!isInVoxel(nextVoxel, motion)) {
      return;
    }

    if (seen.put(nextVoxel, true) == Boolean.TRUE) {
      return;
    }

    putIntoMap(voxelMap, nextVoxel, motion);

    recurse(voxelMap, seen, nextVoxel.minus(horizontal), motion);
    recurse(voxelMap, seen, nextVoxel.plus(horizontal), motion);
    recurse(voxelMap, seen, nextVoxel.minus(vertical), motion);
    recurse(voxelMap, seen, nextVoxel.plus(vertical), motion);
    recurse(voxelMap, seen, nextVoxel.minus(horizontal).minus(vertical), motion);
    recurse(voxelMap, seen, nextVoxel.minus(horizontal).plus(vertical), motion);
    recurse(voxelMap, seen, nextVoxel.plus(horizontal).minus(vertical), motion);
    recurse(voxelMap, seen, nextVoxel.plus(horizontal).plus(vertical), motion);
  }

  private static Vector<Vector<Motion>> reduceCollisionSet(final Vector<Motion> motions) {
    RedBlackTree<Vector2D, Vector<Motion>> voxelMap = new RedBlackTree<>();
    motions.forEach(motion -> drawMotionOnVoxelMap(voxelMap, motion));

    Vector<Vector<Motion>> result = new Vector<>();
    voxelMap.forEach(entry -> {
      if (entry.value.size() > 1) {
        result.append(entry.value);
      }
    });
    return result;
  }

  private static Vector2D voxelHash(final Vector3D position) {
    int quotientX = (int) (position.coordinateX / Constants.GOOD_VOXEL_SIZE);
    int quotientY = (int) (position.coordinateY / Constants.GOOD_VOXEL_SIZE);

    double voxelX = Constants.GOOD_VOXEL_SIZE * quotientX;
    double voxelY = Constants.GOOD_VOXEL_SIZE * quotientY;

    if (position.coordinateX < 0) {
      voxelX -= Constants.GOOD_VOXEL_SIZE;
    }
    if (position.coordinateY < 0) {
      voxelY -= Constants.GOOD_VOXEL_SIZE;
    }

    return new Vector2D(voxelX, voxelY);
  }

  private static void drawMotionOnVoxelMap(
      final RedBlackTree<Vector2D, Vector<Motion>> voxelMap, final Motion motion) {
    RedBlackTree<Vector2D, Boolean> seen = new RedBlackTree<>();
    recurse(voxelMap, seen, voxelHash(motion.positionOne), motion);
  }
}
