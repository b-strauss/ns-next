/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/Mandelbrot.java).
 * Changed for ns-next: innerBenchmarkLoop, benchmark and verifyResult are
 * bench, which runs once at benchmarks/are-we-fast-yet/newspeak/Mandelbrot.ns's size,
 * 750, and checks its result, 50, with that port's message; the checks for
 * upstream's other sizes, 500 and 1, are dropped; the class is public; names
 * are written out.
 */
// This benchmark has been modified based on the SOM benchmark.
//
// Copyright (C) 2004-2013 Brent Fulgham
//
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are met:
//
//   * Redistributions of source code must retain the above copyright notice,
//     this list of conditions and the following disclaimer.
//
//   * Redistributions in binary form must reproduce the above copyright notice,
//     this list of conditions and the following disclaimer in the documentation
//     and/or other materials provided with the distribution.
//
//   * Neither the name of "The Computer Language Benchmarks Game" nor the name
//     of "The Computer Language Shootout Benchmarks" nor the names of its
//     contributors may be used to endorse or promote products derived from this
//     software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
// AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE
// FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
// DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
// CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
// OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
// OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

// The Computer Language Benchmarks Game
// http://benchmarksgame.alioth.debian.org
//
//  contributed by Karl von Laudermann
//  modified by Jeremy Echols
//  modified by Detlef Reichl
//  modified by Joseph LaFata
//  modified by Peter Zotov

// http://benchmarksgame.alioth.debian.org/u64q/program.php?test=mandelbrot&lang=yarv&id=3

public final class Mandelbrot extends Benchmark {

  @Override
  public void bench() {
    int result = mandelbrot(750);
    if (result != 50) {
      throw new IllegalStateException("Mandelbrot: expected 50, got " + result);
    }
  }

  private int mandelbrot(final int size) {
     int sum             = 0;
     int byteAccumulator = 0;
     int bitNumber       = 0;

     int row = 0;

     while (row < size) {
       double imaginaryConstant = (2.0 * row / size) - 1.0;
       int column = 0;

       while (column < size) {
         double realSquared      = 0.0;
         double imaginary        = 0.0;
         double imaginarySquared = 0.0;
         double realConstant = (2.0 * column / size) - 1.5;

         int iteration = 0;
         boolean notDone = true;
         int escape = 0;
         while (notDone && iteration < 50) {
           double real = realSquared - imaginarySquared + realConstant;
           imaginary = 2.0 * real * imaginary + imaginaryConstant;

           // preserve recalculation
           realSquared = real * real;
           imaginarySquared = imaginary * imaginary;

           if (realSquared + imaginarySquared > 4.0) {
             notDone = false;
             escape  = 1;
           }
           iteration += 1;
         }

         byteAccumulator = (byteAccumulator << 1) + escape;
         bitNumber += 1;

         // Code is very similar for these cases, but using separate blocks
         // ensures we skip the shifting when it's unnecessary, which is most cases.
         if (bitNumber == 8) {
           sum ^= byteAccumulator;
           byteAccumulator = 0;
           bitNumber = 0;
         } else if (column == size - 1) {
           byteAccumulator <<= (8 - bitNumber);
           sum ^= byteAccumulator;
           byteAccumulator = 0;
           bitNumber = 0;
         }
         column += 1;
       }
       row += 1;
     }
     return sum;
   }
}
