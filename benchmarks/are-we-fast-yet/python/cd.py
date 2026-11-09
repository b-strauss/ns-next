# Adapted for ns-next from Are We Fast Yet
# (https://github.com/smarr/are-we-fast-yet, commit
# 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Python/cd.py).
# Changed for ns-next: inner_benchmark_loop, benchmark and verify_result are
# bench, which runs _benchmark once at the size of
# benchmarks/are-we-fast-yet/newspeak/CD.ns, 250 aircraft, and checks 10830 collisions,
# raising a RuntimeError with that port's message; the checks for upstream's
# other sizes and the printing of an unknown size's result are dropped; names
# are written out, as in java/cd/. Upstream's Python file carries only the MIT
# notice below, but it derives from Purdue University's CDx, so the BSD-3
# notice after it, copied from the header of benchmarks/are-we-fast-yet/newspeak/CD.ns
# and carried by upstream's JavaScript and SOM versions of CD, follows.

# Copyright (c) 2001-2021 Stefan Marr
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

# Copyright (c) 2001-2010, Purdue University. All rights reserved.
# Copyright (C) 2015 Apple Inc. All rights reserved.
#
# Redistribution and use in source and binary forms, with or without
# modification, are permitted provided that the following conditions are met:
#  * Redistributions of source code must retain the above copyright
#    notice, this list of conditions and the following disclaimer.
#  * Redistributions in binary form must reproduce the above copyright
#    notice, this list of conditions and the following disclaimer in the
#    documentation and/or other materials provided with the distribution.
#  * Neither the name of the Purdue University nor the
#    names of its contributors may be used to endorse or promote products
#    derived from this software without specific prior written permission.
#
# THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
# ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
# WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
# DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER BE LIABLE FOR ANY
# DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
# (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
# LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
# ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
# (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
# SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
from enum import Enum
from math import sqrt, cos, sin

from benchmark import Benchmark
from som.vector import Vector

MIN_X = 0.0
MIN_Y = 0.0
MAX_X = 1000.0
MAX_Y = 1000.0
MIN_Z = 0.0
MAX_Z = 10.0
PROXIMITY_RADIUS = 1.0
GOOD_VOXEL_SIZE = PROXIMITY_RADIUS * 2.0


def _compare_numbers(left, right):
    if left == right:
        return 0

    if left < right:
        return -1

    if left > right:
        return 1

    # We say that NaN is smaller than non-NaN.
    if left == left:  # pylint: disable=comparison-with-itself
        return 1

    return -1


class _Vector2D:
    def __init__(self, coordinate_x, coordinate_y):
        self.coordinate_x = coordinate_x
        self.coordinate_y = coordinate_y

    def plus(self, other):
        return _Vector2D(self.coordinate_x + other.coordinate_x, self.coordinate_y + other.coordinate_y)

    def minus(self, other):
        return _Vector2D(self.coordinate_x - other.coordinate_x, self.coordinate_y - other.coordinate_y)

    def compare_to(self, other):
        result = _compare_numbers(self.coordinate_x, other.coordinate_x)
        if result != 0:
            return result

        return _compare_numbers(self.coordinate_y, other.coordinate_y)


class _Vector3D:
    def __init__(self, coordinate_x, coordinate_y, coordinate_z):
        self.coordinate_x = coordinate_x
        self.coordinate_y = coordinate_y
        self.coordinate_z = coordinate_z

    def plus(self, other):
        return _Vector3D(self.coordinate_x + other.coordinate_x, self.coordinate_y + other.coordinate_y, self.coordinate_z + other.coordinate_z)

    def minus(self, other):
        return _Vector3D(self.coordinate_x - other.coordinate_x, self.coordinate_y - other.coordinate_y, self.coordinate_z - other.coordinate_z)

    def dot(self, other):
        return self.coordinate_x * other.coordinate_x + self.coordinate_y * other.coordinate_y + self.coordinate_z * other.coordinate_z

    def squared_magnitude(self):
        return self.dot(self)

    def magnitude(self):
        return sqrt(self.squared_magnitude())

    def times(self, amount):
        return _Vector3D(self.coordinate_x * amount, self.coordinate_y * amount, self.coordinate_z * amount)


_horizontal = _Vector2D(GOOD_VOXEL_SIZE, 0.0)
_vertical = _Vector2D(0.0, GOOD_VOXEL_SIZE)


class _Color(Enum):
    RED = 1
    BLACK = 2


def _tree_minimum(node):
    current = node
    while current.left is not None:
        current = current.left

    return current


class _Node:
    def __init__(self, key, value):
        self.key = key
        self.value = value
        self.left = None
        self.right = None
        self.parent = None
        self.color = _Color.RED

    def successor(self):
        node = self
        if node.right is not None:
            return _tree_minimum(node.right)

        ancestor = node.parent
        while ancestor is not None and node is ancestor.right:
            node = ancestor
            ancestor = ancestor.parent

        return ancestor


class _Entry:
    def __init__(self, key, value):
        self.key = key
        self.value = value


class _InsertResult:
    def __init__(self, is_new_entry, new_node, old_value):
        self.is_new_entry = is_new_entry
        self.new_node = new_node
        self.old_value = old_value


class _RedBlackTree:
    def __init__(self):
        self._root = None

    def put(self, key, value):
        insertion_result = self._tree_insert(key, value)
        if not insertion_result.is_new_entry:
            return insertion_result.old_value

        node = insertion_result.new_node

        while node is not self._root and node.parent.color is _Color.RED:
            if node.parent is node.parent.parent.left:
                uncle = node.parent.parent.right
                if uncle is not None and uncle.color is _Color.RED:
                    # Case 1
                    node.parent.color = _Color.BLACK
                    uncle.color = _Color.BLACK
                    node.parent.parent.color = _Color.RED
                    node = node.parent.parent
                else:
                    if node is node.parent.right:
                        # Case 2
                        node = node.parent
                        self._left_rotate(node)

                    # Case 3
                    node.parent.color = _Color.BLACK
                    node.parent.parent.color = _Color.RED
                    self._right_rotate(node.parent.parent)

            else:
                # Same as "then" clause with "right" and "left" exchanged.
                uncle = node.parent.parent.left
                if uncle is not None and uncle.color is _Color.RED:
                    # Case 1
                    node.parent.color = _Color.BLACK
                    uncle.color = _Color.BLACK
                    node.parent.parent.color = _Color.RED
                    node = node.parent.parent
                else:
                    if node is node.parent.left:
                        # Case 2
                        node = node.parent
                        self._right_rotate(node)

                    # Case 3
                    node.parent.color = _Color.BLACK
                    node.parent.parent.color = _Color.RED
                    self._left_rotate(node.parent.parent)

        self._root.color = _Color.BLACK
        return None

    def remove(self, key):
        target = self._find_node(key)
        if target is None:
            return None

        # y is the node to be unlinked from the tree.
        if target.left is None or target.right is None:
            spliced = target
        else:
            spliced = target.successor()

        # y is guaranteed to be non-null at this point.
        if spliced.left is not None:
            child = spliced.left
        else:
            child = spliced.right

        # x is the child of y which might potentially replace y in the tree. X might be null at
        # this point.
        if child is not None:
            child.parent = spliced.parent
            child_parent = child.parent
        else:
            child_parent = spliced.parent

        if spliced.parent is None:
            self._root = child
        else:
            if spliced is spliced.parent.left:
                spliced.parent.left = child
            else:
                spliced.parent.right = child

        if spliced is not target:
            if spliced.color is _Color.BLACK:
                self._remove_fixup(child, child_parent)

            spliced.parent = target.parent
            spliced.color = target.color
            spliced.left = target.left
            spliced.right = target.right

            if target.left is not None:
                target.left.parent = spliced
            if target.right is not None:
                target.right.parent = spliced
            if target.parent is not None:
                if target.parent.left == target:
                    target.parent.left = spliced
                else:
                    target.parent.right = spliced
            else:
                self._root = spliced
        elif spliced.color is _Color.BLACK:
            self._remove_fixup(child, child_parent)

        return target.value

    def get(self, key):
        node = self._find_node(key)
        if node is None:
            return None

        return node.value

    def for_each(self, function):
        if self._root is None:
            return

        current = _tree_minimum(self._root)
        while current is not None:
            function(_Entry(current.key, current.value))
            current = current.successor()

    def _find_node(self, key):
        current = self._root
        while current is not None:
            comparison_result = key.compare_to(current.key)
            if comparison_result == 0:
                return current
            if comparison_result < 0:
                current = current.left
            else:
                current = current.right
        return None

    def _tree_insert(self, key, value):
        parent = None
        current = self._root

        while current is not None:
            parent = current
            comparison_result = key.compare_to(current.key)
            if comparison_result < 0:
                current = current.left
            elif comparison_result > 0:
                current = current.right
            else:
                old_value = current.value
                current.value = value
                return _InsertResult(False, None, old_value)

        new_node = _Node(key, value)
        new_node.parent = parent
        if parent is None:
            self._root = new_node
        else:
            if key.compare_to(parent.key) < 0:
                parent.left = new_node
            else:
                parent.right = new_node
        return _InsertResult(True, new_node, None)

    def _left_rotate(self, node):
        pivot = node.right

        # Turn y's left subtree into x's right subtree.
        node.right = pivot.left
        if pivot.left is not None:
            pivot.left.parent = node

        # Link x's parent to y.
        pivot.parent = node.parent
        if node.parent is None:
            self._root = pivot
        else:
            if node is node.parent.left:
                node.parent.left = pivot
            else:
                node.parent.right = pivot

        # Put x on y's left.
        pivot.left = node
        node.parent = pivot

        return pivot

    def _right_rotate(self, node):
        pivot = node.left

        # Turn x's right subtree into y's left subtree.
        node.left = pivot.right
        if pivot.right is not None:
            pivot.right.parent = node

        # Link y's parent to x;
        pivot.parent = node.parent
        if node.parent is None:
            self._root = pivot
        else:
            if node is node.parent.left:
                node.parent.left = pivot
            else:
                node.parent.right = pivot

        pivot.right = node
        node.parent = pivot

        return pivot

    def _remove_fixup(self, child, child_parent):
        while child is not self._root and (child is None or child.color is _Color.BLACK):
            if child is child_parent.left:
                # Note: the text points out that w cannot be null. The reason is not obvious from
                # simply looking at the code; it comes about from the properties of the red-black
                # tree.
                sibling = child_parent.right
                if sibling.color is _Color.RED:
                    # Case 1
                    sibling.color = _Color.BLACK
                    child_parent.color = _Color.RED
                    self._left_rotate(child_parent)
                    sibling = child_parent.right

                if (sibling.left is None or sibling.left.color is _Color.BLACK) and (
                    sibling.right is None or sibling.right.color is _Color.BLACK
                ):
                    # Case 2
                    sibling.color = _Color.RED
                    child = child_parent
                    child_parent = child.parent
                else:
                    if sibling.right is None or sibling.right.color is _Color.BLACK:
                        # Case 3
                        sibling.left.color = _Color.BLACK
                        sibling.color = _Color.RED
                        self._right_rotate(sibling)
                        sibling = child_parent.right

                    # Case 4
                    sibling.color = child_parent.color
                    child_parent.color = _Color.BLACK
                    if sibling.right is not None:
                        sibling.right.color = _Color.BLACK

                    self._left_rotate(child_parent)
                    child = self._root
                    child_parent = child.parent
            else:
                # Same as "then" clause with "right" and "left" exchanged.
                sibling = child_parent.left
                if sibling.color is _Color.RED:
                    # Case 1
                    sibling.color = _Color.BLACK
                    child_parent.color = _Color.RED
                    self._right_rotate(child_parent)
                    sibling = child_parent.left

                if (sibling.right is None or sibling.right.color is _Color.BLACK) and (
                    sibling.left is None or sibling.left.color is _Color.BLACK
                ):
                    # Case 2
                    sibling.color = _Color.RED
                    child = child_parent
                    child_parent = child.parent
                else:
                    if sibling.left is None or sibling.left.color is _Color.BLACK:
                        # Case 3
                        sibling.right.color = _Color.BLACK
                        sibling.color = _Color.RED
                        self._left_rotate(sibling)
                        sibling = child_parent.left

                    # Case 4
                    sibling.color = child_parent.color
                    child_parent.color = _Color.BLACK
                    if sibling.left is not None:
                        sibling.left.color = _Color.BLACK

                    self._right_rotate(child_parent)
                    child = self._root
                    child_parent = child.parent

        if child is not None:
            child.color = _Color.BLACK


class _CallSign:
    def __init__(self, value):
        self._value = value

    def compare_to(self, other):
        if self._value == other._value:  # pylint: disable=protected-access
            return 0
        if self._value < other._value:  # pylint: disable=protected-access
            return -1
        return 1


class _Collision:
    def __init__(self, aircraft_a, aircraft_b, position):
        self.aircraft_a = aircraft_a
        self.aircraft_b = aircraft_b
        self.position = position


class _CollisionDetector:
    def __init__(self):
        self._state = _RedBlackTree()

    def handle_new_frame(self, frame):
        motions = Vector()
        seen = _RedBlackTree()

        def each(aircraft):
            old_position = self._state.put(aircraft.call_sign, aircraft.position)
            new_position = aircraft.position
            seen.put(aircraft.call_sign, True)

            if old_position is None:
                # Treat newly introduced aircraft as if they were stationary.
                old_position = new_position

            motions.append(_Motion(aircraft.call_sign, old_position, new_position))

        frame.for_each(each)

        # Remove aircraft that are no longer present.
        to_remove = Vector()

        def for_removal(entry):
            if not seen.get(entry.key):
                to_remove.append(entry.key)

        self._state.for_each(for_removal)

        to_remove.for_each(self._state.remove)

        all_reduced = _reduce_collision_set(motions)
        collisions = Vector()

        def find_collisions(reduced):
            for first in range(reduced.size()):
                motion_one = reduced.at(first)
                for second in range(first + 1, reduced.size()):
                    motion_two = reduced.at(second)
                    collision = motion_one.find_intersection(motion_two)
                    if collision is not None:
                        collisions.append(
                            _Collision(motion_one.call_sign, motion_two.call_sign, collision)
                        )

        all_reduced.for_each(find_collisions)

        return collisions


_positive_infinity = float("inf")
_negative_infinity = float("-inf")


def _is_in_voxel(voxel, motion):
    if voxel.coordinate_x > MAX_X or voxel.coordinate_x < MIN_X or voxel.coordinate_y > MAX_Y or voxel.coordinate_y < MIN_Y:
        return False

    start = motion.position_one
    finish = motion.position_two

    voxel_size = GOOD_VOXEL_SIZE
    radius = PROXIMITY_RADIUS / 2.0

    voxel_x = voxel.coordinate_x
    start_x = start.coordinate_x
    delta_x = finish.coordinate_x - start.coordinate_x

    voxel_y = voxel.coordinate_y
    start_y = start.coordinate_y
    delta_y = finish.coordinate_y - start.coordinate_y

    if delta_x == 0.0:
        low_x = _negative_infinity if (voxel_x - radius - start_x) < 0.0 else _positive_infinity
        high_x = _negative_infinity if (voxel_x + voxel_size + radius - start_x) < 0.0 else _positive_infinity
    else:
        low_x = (voxel_x - radius - start_x) / delta_x
        high_x = (voxel_x + voxel_size + radius - start_x) / delta_x

    if delta_x < 0.0:
        low_x, high_x = high_x, low_x

    if delta_y == 0.0:
        low_y = _negative_infinity if (voxel_y - radius - start_y) < 0.0 else _positive_infinity
        high_y = _negative_infinity if (voxel_y + voxel_size + radius - start_y) < 0.0 else _positive_infinity
    else:
        low_y = (voxel_y - radius - start_y) / delta_y
        high_y = (voxel_y + voxel_size + radius - start_y) / delta_y

    if delta_y < 0.0:
        low_y, high_y = high_y, low_y

    return (
        (
            (delta_x == 0.0 and voxel_x <= start_x + radius and start_x - radius <= voxel_x + voxel_size)
            or (low_x <= 1.0 and 1.0 <= high_x)  # no motion in x
            or (low_x <= 0.0 and 0.0 <= high_x)
            or (0.0 <= low_x and high_x <= 1.0)
        )
        and (
            (delta_y == 0.0 and voxel_y <= start_y + radius and start_y - radius <= voxel_y + voxel_size)
            or (  # no motion in y
                (low_y <= 1.0 and 1.0 <= high_y)
                or (low_y <= 0.0 and 0.0 <= high_y)
                or (0.0 <= low_y and high_y <= 1.0)
            )
        )
        and (
            delta_x == 0.0
            or delta_y == 0.0
            or (low_y <= high_x and high_x <= high_y)  # no motion in x or y or both
            or (low_y <= low_x and low_x <= high_y)
            or (low_x <= low_y and high_y <= high_x)
        )
    )


def _put_into_map(voxel_map, voxel, motion):
    array = voxel_map.get(voxel)
    if array is None:
        array = Vector()
        voxel_map.put(voxel, array)
    array.append(motion)


def _recurse(voxel_map, seen, next_voxel, motion):
    if not _is_in_voxel(next_voxel, motion):
        return

    if seen.put(next_voxel, True):
        return

    _put_into_map(voxel_map, next_voxel, motion)

    _recurse(voxel_map, seen, next_voxel.minus(_horizontal), motion)
    _recurse(voxel_map, seen, next_voxel.plus(_horizontal), motion)
    _recurse(voxel_map, seen, next_voxel.minus(_vertical), motion)
    _recurse(voxel_map, seen, next_voxel.plus(_vertical), motion)
    _recurse(voxel_map, seen, next_voxel.minus(_horizontal).minus(_vertical), motion)
    _recurse(voxel_map, seen, next_voxel.minus(_horizontal).plus(_vertical), motion)
    _recurse(voxel_map, seen, next_voxel.plus(_horizontal).minus(_vertical), motion)
    _recurse(voxel_map, seen, next_voxel.plus(_horizontal).plus(_vertical), motion)


def _reduce_collision_set(motions):
    voxel_map = _RedBlackTree()
    motions.for_each(lambda motion: _draw_motion_on_voxel_map(voxel_map, motion))

    result = Vector()

    def each(entry):
        if entry.value.size() > 1:
            result.append(entry.value)

    voxel_map.for_each(each)
    return result


def _voxel_hash(position):
    quotient_x = position.coordinate_x // GOOD_VOXEL_SIZE
    quotient_y = position.coordinate_y // GOOD_VOXEL_SIZE

    voxel_x = GOOD_VOXEL_SIZE * quotient_x
    voxel_y = GOOD_VOXEL_SIZE * quotient_y

    if position.coordinate_x < 0.0:
        voxel_x -= GOOD_VOXEL_SIZE

    if position.coordinate_y < 0.0:
        voxel_y -= GOOD_VOXEL_SIZE

    return _Vector2D(voxel_x, voxel_y)


def _draw_motion_on_voxel_map(voxel_map, motion):
    seen = _RedBlackTree()
    _recurse(voxel_map, seen, _voxel_hash(motion.position_one), motion)


class _Motion:
    def __init__(self, call_sign, position_one, position_two):
        self.call_sign = call_sign
        self.position_one = position_one
        self.position_two = position_two

    def delta(self):
        return self.position_two.minus(self.position_one)

    def find_intersection(self, other):
        initial_one = self.position_one
        initial_two = other.position_one
        vector_one = self.delta()
        vector_two = other.delta()
        radius = PROXIMITY_RADIUS

        # this test is not geometrical 3-d intersection test,
        # it takes the fact that the aircraft move
        # into account ; so it is more like a 4d test
        # (it assumes that both of the aircraft have a constant speed over the tested interval)

        # we thus have two points, each of them moving on its line segment at constant speed ;
        # we are looking for times when the distance between these two points is smaller than r

        # vec1 is vector of aircraft 1
        # vec2 is vector of aircraft 2

        # a = (V2 - V1)^T * (V2 - V1)
        squared_speed = vector_two.minus(vector_one).squared_magnitude()

        if squared_speed != 0.0:
            # we are first looking for instances of time when the planes are
            # exactly r from each other at least one plane is moving ;
            # if the planes are moving in parallel, they do not have constant speed

            # if the planes are moving in parallel, then
            #   if the faster starts behind the slower, we can have 2, 1, or 0 solutions
            #   if the faster plane starts in front of the slower, we can have 0 or 1 solutions

            # if the planes are not moving in parallel, then

            # point P1 = I1 + vV1
            # point P2 = I2 + vV2
            #   - looking for v, such that dist(P1,P2) = || P1 - P2 || = r

            # it follows that || P1 - P2 || = sqrt( < P1-P2, P1-P2 > )
            #   0 = -r^2 + < P1 - P2, P1 - P2 >
            #  from properties of dot product
            #   0 = -r^2 + <I1-I2,I1-I2> + v * 2<I1-I2, V1-V2> + v^2 *<V1-V2,V1-V2>
            #   so we calculate a, b, c - and solve the quadratic equation
            #   0 = c + bv + av^2

            # b = 2 * <I1-I2, V1-V2>
            linear = 2.0 * initial_one.minus(initial_two).dot(vector_one.minus(vector_two))

            # c = -r^2 + (I2 - I1)^T * (I2 - I1)
            constant = -radius * radius + initial_two.minus(initial_one).squared_magnitude()

            discriminant = linear * linear - 4.0 * squared_speed * constant
            if discriminant < 0.0:
                return None

            time_one = (-linear - sqrt(discriminant)) / (2.0 * squared_speed)
            time_two = (-linear + sqrt(discriminant)) / (2.0 * squared_speed)

            if time_one <= time_two and (
                (time_one <= 1.0 and 1.0 <= time_two)
                or (time_one <= 0.0 and 0.0 <= time_two)
                or (0.0 <= time_one and time_two <= 1.0)
            ):
                # Pick a good "time" at which to report the collision.
                if time_one <= 0.0:
                    # The collision started before this frame. Report it at the start of the frame.
                    time = 0.0
                else:
                    # The collision started during this frame. Report it at that moment.
                    time = time_one

                result_one = initial_one.plus(vector_one.times(time))
                result_two = initial_two.plus(vector_two.times(time))

                result = result_one.plus(result_two).times(0.5)
                if (
                    result.coordinate_x >= MIN_X
                    and result.coordinate_x <= MAX_X
                    and result.coordinate_y >= MIN_Y
                    and result.coordinate_y <= MAX_Y
                    and result.coordinate_z >= MIN_Z
                    and result.coordinate_z <= MAX_Z
                ):
                    return result

            return None

        # the planes have the same speeds and are moving in parallel (or they are not moving at all)
        # they  thus have the same distance all the time ; we calculate it from the initial point

        # dist = || i2 - i1 || = sqrt(  ( i2 - i1 )^T * ( i2 - i1 ) )
        distance = initial_two.minus(initial_one).magnitude()
        if distance <= radius:
            return initial_one.plus(initial_two).times(0.5)

        return None


class _Aircraft:
    def __init__(self, call_sign, position):
        self.call_sign = call_sign
        self.position = position


class _Simulator:
    def __init__(self, aircraft_count):
        self._aircraft = Vector()
        for index in range(aircraft_count):
            self._aircraft.append(_CallSign(index))

    def simulate(self, time):
        frame = Vector()
        for index in range(0, self._aircraft.size(), 2):
            frame.append(
                _Aircraft(
                    self._aircraft.at(index),
                    _Vector3D(time, cos(time) * 2.0 + index * 3.0, 10.0),
                )
            )

            frame.append(
                _Aircraft(
                    self._aircraft.at(index + 1),
                    _Vector3D(time, sin(time) * 2.0 + index * 3.0, 10.0),
                )
            )
        return frame


class CD(Benchmark):
    @staticmethod
    def _benchmark(aircraft_count):
        frame_count = 200

        simulator = _Simulator(aircraft_count)
        detector = _CollisionDetector()

        actual_collisions = 0

        for frame in range(frame_count):
            time = frame / 10.0
            collisions = detector.handle_new_frame(simulator.simulate(time))
            actual_collisions += collisions.size()

        return actual_collisions

    def bench(self):
        collisions = self._benchmark(250)
        if collisions != 10830:
            raise RuntimeError("CD: expected 10830, got " + str(collisions))
