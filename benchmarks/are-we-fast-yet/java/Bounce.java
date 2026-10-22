/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Java/src/Bounce.java).
 * Changed for ns-next: benchmark and verifyResult are bench, which runs once
 * and checks its own result, with the message of
 * benchmarks/are-we-fast-yet/Bounce.ns; names are written out, and the four
 * if statements of bounce, too long for one line each with those names, are
 * spread over several.
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
import java.util.Arrays;

import som.Random;


public final class Bounce extends Benchmark {

  private static class Ball {
    private int horizontal;
    private int vertical;
    private int horizontalVelocity;
    private int verticalVelocity;

    Ball(final Random random) {
      horizontal = random.next() % 500;
      vertical = random.next() % 500;
      horizontalVelocity = (random.next() % 300) - 150;
      verticalVelocity = (random.next() % 300) - 150;
    }

    public boolean bounce() {
      int horizontalLimit = 500;
      int verticalLimit = 500;
      boolean bounced = false;

      horizontal += horizontalVelocity;
      vertical += verticalVelocity;
      if (horizontal > horizontalLimit) {
        horizontal = horizontalLimit;
        horizontalVelocity = 0 - Math.abs(horizontalVelocity);
        bounced = true;
      }
      if (horizontal < 0) {
        horizontal = 0;
        horizontalVelocity = Math.abs(horizontalVelocity);
        bounced = true;
      }
      if (vertical > verticalLimit) {
        vertical = verticalLimit;
        verticalVelocity = 0 - Math.abs(verticalVelocity);
        bounced = true;
      }
      if (vertical < 0) {
        vertical = 0;
        verticalVelocity = Math.abs(verticalVelocity);
        bounced = true;
      }
      return bounced;
    }
  }

  @Override
  public void bench() {
    Random random = new Random();

    int ballCount = 100;
    int bounces   = 0;
    Ball[] balls  = new Ball[ballCount];

    Arrays.setAll(balls, index -> new Ball(random));

    for (int index = 0; index < 50; index++) {
      for (Ball ball : balls) {
        if (ball.bounce()) {
          bounces += 1;
        }
      }
    }
    if (bounces != 1331) {
      throw new IllegalStateException("Bounce: expected 1331, got " + bounces);
    }
  }
}
