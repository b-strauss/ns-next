/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/som/Vector.java).
 * Changed for ns-next: names are written out.
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
package som;

import java.util.Arrays;
import java.util.Comparator;

/**
 * Porting notes:
 *  - does not use an explicit array bounds check, because Java already does
 *    that. Don't see a point in doing it twice.
 */
public class Vector<E> {
  private Object[] storage;
  private int firstIndex;
  private int lastIndex;

  public static <E> Vector<E> with(final E element) {
    Vector<E> vector = new Vector<E>(1);
    vector.append(element);
    return vector;
  }

  public Vector(final int size) {
    storage = size == 0 ? null : new Object[size];
  }

  public Vector() { }

  @SuppressWarnings("unchecked")
  public E at(final int index) {
    if (storage == null || index >= storage.length) {
      return null;
    }
    return (E) storage[index];
  }

  public void atPut(final int index, final E value) {
    if (storage == null) {
      storage = new Object[Math.max(index + 1, Constants.INITIAL_SIZE)];
    } else if (index >= storage.length) {
      int newLength = storage.length;
      while (newLength <= index) {
        newLength *= 2;
      }
      storage = Arrays.copyOf(storage, newLength);
    }
    storage[index] = value;
    if (lastIndex < index + 1) {
      lastIndex = index + 1;
    }
  }

  public void append(final E element) {
    if (storage == null) {
      storage = new Object[Constants.INITIAL_SIZE];
    } else if (lastIndex >= storage.length) {
      // Need to expand capacity first
      storage = Arrays.copyOf(storage, 2 * storage.length);
    }

    storage[lastIndex] = element;
    lastIndex++;
  }

  public boolean isEmpty() {
    return lastIndex == firstIndex;
  }

  @SuppressWarnings("unchecked")
  public void forEach(final ForEachInterface<E> function) {
    for (int index = firstIndex; index < lastIndex; index++) {
      function.apply((E) storage[index]);
    }
  }

  @SuppressWarnings("unchecked")
  public boolean hasSome(final TestInterface<E> function) {
    for (int index = firstIndex; index < lastIndex; index++) {
      if (function.test((E) storage[index])) {
        return true;
      }
    }
    return false;
  }

  @SuppressWarnings("unchecked")
  public E getOne(final TestInterface<E> function) {
    for (int index = firstIndex; index < lastIndex; index++) {
      E element = (E) storage[index];
      if (function.test(element)) {
        return element;
      }
    }
    return null;
  }

  @SuppressWarnings("unchecked")
  public E first() {
    if (isEmpty()) {
      return null;
    }
    return (E) storage[firstIndex];
  }

  @SuppressWarnings("unchecked")
  public E removeFirst() {
    if (isEmpty()) {
      return null;
    }
    firstIndex++;
    return (E) storage[firstIndex - 1];
  }

  public boolean remove(final E object) {
    if (storage == null || isEmpty()) {
      return false;
    }

    Object[] newArray = new Object[capacity()];
    int[] newLast     = new int[] {0};
    boolean[] found   = new boolean[] {false};

    forEach(each -> {
      if (each == object) {
        found[0] = true;
      } else {
        newArray[newLast[0]] = each;
        newLast[0]++;
    }});

    storage    = newArray;
    lastIndex  = newLast[0];
    firstIndex = 0;
    return found[0];
  }

  public void removeAll() {
    firstIndex = 0;
    lastIndex = 0;
    if (storage != null) {
      storage = new Object[storage.length];
    }
  }

  public int size() {
    return lastIndex - firstIndex;
  }

  public int capacity() {
    return storage == null ? 0 : storage.length;
  }

  public void sort(final Comparator<E> comparator) {
    if (size() > 0) {
      sort(firstIndex, lastIndex - 1, comparator);
    }
  }

  @SuppressWarnings("unchecked")
  private void sort(final int first, final int last, final Comparator<E> comparator) {
    if (comparator == null) {
      defaultSort(first, last);
    }

    int length = last + 1 - first;
    if (length <= 1) {
      return;
    }

    E firstElement = (E) storage[first];
    E lastElement = (E) storage[last];

    if (comparator.compare(firstElement, lastElement) > 0) {
      swap(storage, first, last);
      E temp = firstElement;
      firstElement = lastElement;
      lastElement = temp;
    }

    if (length > 2) {
      int middle = (first + last) / 2;
      E middleElement = (E) storage[middle];

      if (comparator.compare(firstElement, middleElement) <= 0) {
        if (comparator.compare(middleElement, lastElement) > 0) {
          swap(storage, last, middle);
          middleElement = lastElement;
        }
      } else {
        swap(storage, first, middle);
        middleElement = firstElement;
      }

      if (length > 3) {
        int left = first;
        int right = last - 1;

        while (true) {
          while (left <= right && comparator.compare(middleElement, (E) storage[right]) <= 0) {
            right -= 1;
          }

          left += 1;
          while (left <= right && comparator.compare((E) storage[left], middleElement) <= 0) {
            left += 1;
          }

          if (left > right) {
            break;
          }
          swap(storage, left, right);
        }

        sort(first, right, comparator);
        sort(left, last, comparator);
      }
    }
  }

  private static void swap(final Object[] array, final int first, final int second) {
    throw new NotImplemented();
  }

  private void defaultSort(final int first, final int last) {
    throw new NotImplemented();
  }
}
