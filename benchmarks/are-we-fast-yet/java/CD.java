/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/CD.java).
 * Changed for ns-next: innerBenchmarkLoop, benchmark() and verifyResult are
 * bench, which runs benchmark once at the size of
 * benchmarks/are-we-fast-yet/CD.ns, 250 aircraft, and checks 10830 collisions
 * with that port's message; the checks for upstream's other sizes and the
 * printing of an unknown size's result are dropped; names are written out.
 * That port's other departures concern the Newspeak sources it adapts and
 * Primordial Soup's protocol; none is needed here. Upstream's Java file
 * carries only the MIT notice below, but its benchmark loop, like the
 * files of cd/, derives from Purdue University's CDx, so the BSD-3 notice
 * after it, copied from the header of benchmarks/are-we-fast-yet/CD.ns and
 * carried by upstream's JavaScript and SOM versions of CD, follows.
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
