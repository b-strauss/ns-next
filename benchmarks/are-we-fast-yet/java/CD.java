/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/CD.java).
 * Changed for ns-next: innerBenchmarkLoop, benchmark() and verifyResult are
 * bench, which runs benchmark once at the size of
 * benchmarks/are-we-fast-yet/CD.ns, 250 aircraft, and checks 10830 collisions
 * with that port's message; the checks for upstream's other sizes and the
 * printing of an unknown size's result are dropped; names are written out.
 * That port's other departures answer Primordial Soup's protocol and have no
 * counterpart here.
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
import cd.Collision;
import cd.CollisionDetector;
import cd.Simulator;
import som.Vector;

public final class CD extends Benchmark {

  private int benchmark(final int aircraftCount) {
    int frameCount = 200;

    Simulator simulator = new Simulator(aircraftCount);
    CollisionDetector detector = new CollisionDetector();

    int actualCollisions = 0;

    for (int frame = 0; frame < frameCount; frame++) {
      double time = frame / 10.0;
      Vector<Collision> collisions = detector.handleNewFrame(simulator.simulate(time));
      actualCollisions += collisions.size();
    }

    return actualCollisions;
  }

  @Override
  public void bench() {
    int collisions = benchmark(250);
    if (collisions != 10830) {
      throw new IllegalStateException("CD: expected 10830, got " + collisions);
    }
  }
}
