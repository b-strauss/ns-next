# Adapted for ns-next from Are We Fast Yet
# (https://github.com/smarr/are-we-fast-yet, commit
# 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Python/som/vector.py).
# Changed for ns-next: names are written out; remove_last, which upstream's
# Vector lacks, is added for deltablueton80, whose port takes its work lists
# from the end, as the Java Vector beside it adds removeLast.

# This code is based on the SOM class library.
#
# Copyright (c) 2001-2021 see AUTHORS.md file
#
# Permission is hereby granted, free of charge, to any person obtaining a copy
# of this software and associated documentation files (the 'Software'), to deal
# in the Software without restriction, including without limitation the rights
# to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
# copies of the Software, and to permit persons to whom the Software is
# furnished to do so, subject to the following conditions:
#
# The above copyright notice and this permission notice shall be included in
# all copies or substantial portions of the Software.
#
# THE SOFTWARE IS PROVIDED 'AS IS', WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
# IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
# FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
# AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
# LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
# OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
# THE SOFTWARE.
from som.constants import INITIAL_SIZE


def vector_with(element):
    vector = Vector(1)
    vector.append(element)
    return vector


# Porting notes:
#  - does not use an explicit array bounds check, because Java already does
#    that. Don't see a point in doing it twice.
class Vector:
    def __init__(self, size=0):
        self._storage = None if size == 0 else [None] * size
        self._first_index = 0
        self._last_index = 0

    def at(self, index):
        if self._storage is None or index >= len(self._storage):
            return None
        return self._storage[index]

    def at_put(self, index, value):
        if self._storage is None:
            self._storage = [None] * max(index + 1, INITIAL_SIZE)
        elif index >= len(self._storage):
            new_length = len(self._storage)
            while new_length <= index:
                new_length *= 2

            new_storage = [None] * new_length
            for position in range(len(self._storage)):
                new_storage[position] = self._storage[position]
            self._storage = new_storage

        self._storage[index] = value
        if self._last_index < index + 1:
            self._last_index = index + 1

    def append(self, element):
        if self._storage is None:
            self._storage = [None] * INITIAL_SIZE
        elif self._last_index >= len(self._storage):
            # Need to expand capacity first
            new_storage = [None] * (2 * len(self._storage))
            for index in range(len(self._storage)):
                new_storage[index] = self._storage[index]
            self._storage = new_storage

        self._storage[self._last_index] = element
        self._last_index += 1

    def is_empty(self):
        return self._last_index == self._first_index

    def for_each(self, function):
        for index in range(self._first_index, self._last_index):
            function(self._storage[index])

    def has_some(self, function):
        for index in range(self._first_index, self._last_index):
            if function(self._storage[index]):
                return True
        return False

    def get_one(self, function):
        for index in range(self._first_index, self._last_index):
            element = self._storage[index]
            if function(element):
                return element
        return None

    def first(self):
        if self.is_empty():
            return None
        return self._storage[self._first_index]

    def remove_first(self):
        if self.is_empty():
            return None
        self._first_index += 1
        return self._storage[self._first_index - 1]

    def remove_last(self):
        if self.is_empty():
            return None
        self._last_index -= 1
        return self._storage[self._last_index]

    def remove(self, element):
        if self._storage is None or self.is_empty():
            return False

        new_array = [None] * self.capacity()
        new_last = 0
        found = False

        def each(candidate):
            nonlocal new_last
            nonlocal found
            if candidate is element:
                found = True
            else:
                new_array[new_last] = candidate
                new_last += 1

        self.for_each(each)

        self._storage = new_array
        self._last_index = new_last
        self._first_index = 0
        return found

    def remove_all(self):
        self._first_index = 0
        self._last_index = 0

        if self._storage is not None:
            self._storage = [None] * len(self._storage)

    def size(self):
        return self._last_index - self._first_index

    def capacity(self):
        return 0 if self._storage is None else len(self._storage)

    def sort(self, comparator):
        if self.size() > 0:
            self._sort(self._first_index, self._last_index - 1, comparator)

    def _sort(self, first, last, comparator):
        if comparator is None:
            self._default_sort(first, last)

        length = last + 1 - first
        if length <= 1:
            return

        first_element = self._storage[first]
        last_element = self._storage[last]

        if comparator.compare(first_element, last_element) > 0:
            self._swap(self._storage, first, last)
            first_element, last_element = last_element, first_element

        if length > 2:
            middle = (first + last) // 2
            middle_element = self._storage[middle]

            if comparator.compare(first_element, middle_element) <= 0:
                if comparator.compare(middle_element, last_element) > 0:
                    self._swap(self._storage, last, middle)
                    middle_element = last_element
            else:
                self._swap(self._storage, first, middle)
                middle_element = first_element

            if length > 3:
                left = first
                right = last - 1

                while True:
                    while left <= right and comparator.compare(middle_element, self._storage[right]) <= 0:
                        right -= 1

                    left += 1
                    while left <= right and comparator.compare(self._storage[left], middle_element) <= 0:
                        left += 1

                    if left > right:
                        break

                    self._swap(self._storage, left, right)

                self._sort(first, right, comparator)
                self._sort(left, last, comparator)

    def _swap(self, storage, first, second):
        raise NotImplementedError()

    def _default_sort(self, first, last):
        raise NotImplementedError()
