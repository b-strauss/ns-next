/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Java/src/List.java).
 * Changed for ns-next: benchmark and verifyResult are bench, which runs once
 * and checks its own result, with the message of
 * benchmarks/are-we-fast-yet/List.ns; names are written out.
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
public final class List extends Benchmark {

  private static class Element {
    private Object value;
    private Element next;

    Element(final Object value) {
      this.value = value;
    }

    public int length() {
      if (next == null) {
        return 1;
      } else {
        return 1 + next.length();
      }
    }

    public Object  getValue()                     { return value; }
    public void    setValue(final Object value)   { this.value = value; }
    public Element getNext()                      { return next; }
    public void    setNext(final Element element) { next = element; }
  }

  @Override
  public void bench() {
    Element result = tail(makeList(15), makeList(10), makeList(6));
    int length = result.length();
    if (length != 10) {
      throw new IllegalStateException("List: expected 10, got " + length);
    }
  }

  private Element makeList(final int length) {
    if (length == 0) { return null; } else {
      Element element = new Element(length);
      element.setNext(makeList(length - 1));
      return element;
    }
  }

  private boolean isShorterThan(final Element first, final Element second) {
    Element firstTail = first;
    Element secondTail = second;

    while (secondTail != null) {
      if (firstTail == null) { return true; }
      firstTail = firstTail.getNext();
      secondTail = secondTail.getNext();
    }
    return false;
  }

  private Element tail(final Element first, final Element second, final Element third) {
    if (isShorterThan(second, first)) {
      return tail(tail(first.getNext(), second, third),
             tail(second.getNext(), third, first),
             tail(third.getNext(), first, second));
    } else {
      return third;
    }
  }
}
