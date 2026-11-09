# Adapted for ns-next from Are We Fast Yet
# (https://github.com/smarr/are-we-fast-yet, commit
# 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Python/havlak.py).
# Changed for ns-next: inner_benchmark_loop, benchmark and verify_result are
# bench, which runs once at the size of benchmarks/are-we-fast-yet/Havlak.ns,
# 1500 dummy loops, 50 loop-finding iterations and 10, 10 and 5 nested loops,
# as upstream runs it at that size, and checks 6102 loops in 5213 nodes,
# raising a RuntimeError with that port's message; the checks for upstream's
# other sizes and the printing of an unknown size's result are dropped; names
# are written out, as in java/havlak/, the basic block kinds included
# (BB_NONHEADER is NON_HEADER, BB_SELF is SELF_LOOP, and so on).

# Adapted based on SOM benchmark.
# Copyright 2011 Google Inc.
#
#     Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
#     You may obtain a copy of the License at
#
# http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
#     WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
#     See the License for the specific language governing permissions and
#         limitations under the License.
from enum import Enum

import sys

from benchmark import Benchmark
from som.identity_dictionary import IdentityDictionary
from som.identity_set import IdentitySet
from som.set import Set
from som.vector import Vector

# Havlak needs more stack space in CPython
sys.setrecursionlimit(1500)


class Havlak(Benchmark):
    def bench(self):
        result = _LoopTesterApp().main(1500, 50, 10, 10, 5)
        if result[0] != 6102 or result[1] != 5213:
            raise RuntimeError(
                "Havlak: expected 6102 loops and 5213 nodes, got "
                + str(result[0])
                + " and "
                + str(result[1])
            )


class _BasicBlock:
    def __init__(self, name):
        self._name = name
        self.in_edges = Vector(2)
        self.out_edges = Vector(2)

    def get_predecessor_count(self):
        return self.in_edges.size()

    def add_out_edge(self, to_block):
        self.out_edges.append(to_block)

    def add_in_edge(self, from_block):
        self.in_edges.append(from_block)

    def custom_hash(self):
        return self._name


class _BasicBlockEdge:
    def __init__(self, control_flow_graph, from_name, to_name):
        self._from_block = control_flow_graph.create_node(from_name)
        self._to_block = control_flow_graph.create_node(to_name)

        self._from_block.add_out_edge(self._to_block)
        self._to_block.add_in_edge(self._from_block)

        control_flow_graph.add_edge(self)


class _ControlFlowGraph:
    def __init__(self):
        self.start_basic_block = None
        self.basic_blocks = Vector()
        self._edge_list = Vector()

    def create_node(self, name):
        if self.basic_blocks.at(name):
            node = self.basic_blocks.at(name)
        else:
            node = _BasicBlock(name)
            self.basic_blocks.at_put(name, node)

        if self.get_node_count() == 1:
            self.start_basic_block = node
        return node

    def add_edge(self, edge):
        self._edge_list.append(edge)

    def get_node_count(self):
        return self.basic_blocks.size()


class _LoopStructureGraph:
    def __init__(self):
        self._loop_counter = 0
        self._loops = Vector()
        self._root = _SimpleLoop(None, True)
        self._root.set_nesting_level(0)
        self._root.counter = self._loop_counter
        self._loop_counter += 1
        self._loops.append(self._root)

    def create_new_loop(self, basic_block, is_reducible):
        loop = _SimpleLoop(basic_block, is_reducible)
        loop.counter = self._loop_counter
        self._loop_counter += 1
        self._loops.append(loop)
        return loop

    def calculate_nesting_level(self):
        def each(loop):
            if not loop.is_root:
                if loop.parent is None:
                    loop.set_parent(self._root)

        self._loops.for_each(each)
        self._calculate_nesting_level_recursive(self._root, 0)

    def _calculate_nesting_level_recursive(self, loop, depth):
        loop.depth_level = depth

        def each(child):
            self._calculate_nesting_level_recursive(child, depth + 1)
            loop.set_nesting_level(max(loop.nesting_level, 1 + child.nesting_level))

        loop.children.for_each(each)

    def get_loop_count(self):
        return self._loops.size()


class _SimpleLoop:
    def __init__(self, basic_block, is_reducible):
        self._is_reducible = is_reducible
        self.parent = None
        self.is_root = False
        self.nesting_level = 0
        self._depth_level = 0
        self._counter = 0
        self._basic_blocks = IdentitySet()
        self.children = IdentitySet()

        if basic_block is not None:
            self._basic_blocks.add(basic_block)

        self._header = basic_block

    def add_node(self, basic_block):
        self._basic_blocks.add(basic_block)

    def add_child_loop(self, loop):
        self.children.add(loop)

    def set_parent(self, parent):
        self.parent = parent
        self.parent.add_child_loop(self)

    def set_nesting_level(self, level):
        self.nesting_level = level
        if level == 0:
            self.is_root = True


class _UnionFindNode:
    def __init__(self):
        self.parent = None
        self.basic_block = None
        self.depth_first_number = 0
        self.loop = None

    def initialize_node(self, basic_block, depth_first_number):
        self.parent = self
        self.basic_block = basic_block
        self.depth_first_number = depth_first_number
        self.loop = None

    def find_set(self):
        node_list = Vector()

        node = self
        while node is not node.parent:
            if node.parent is not node.parent.parent:
                node_list.append(node)
            node = node.parent

        node_list.for_each(lambda each: each.union(self.parent))
        return node

    def union(self, basic_block):
        self.parent = basic_block


class _LoopTesterApp:
    def __init__(self):
        self._control_flow_graph = _ControlFlowGraph()
        self._loop_structure_graph = _LoopStructureGraph()
        self._control_flow_graph.create_node(0)

    def _build_diamond(self, start):
        basic_block = start
        _BasicBlockEdge(self._control_flow_graph, basic_block, basic_block + 1)
        _BasicBlockEdge(self._control_flow_graph, basic_block, basic_block + 2)
        _BasicBlockEdge(self._control_flow_graph, basic_block + 1, basic_block + 3)
        _BasicBlockEdge(self._control_flow_graph, basic_block + 2, basic_block + 3)
        return basic_block + 3

    def _build_connect(self, start, end_):
        _BasicBlockEdge(self._control_flow_graph, start, end_)

    def _build_straight(self, start, length):
        for index in range(length):
            self._build_connect(start + index, start + index + 1)
        return start + length

    def _build_base_loop(self, from_):
        header = self._build_straight(from_, 1)
        first_diamond = self._build_diamond(header)
        between = self._build_straight(first_diamond, 1)
        second_diamond = self._build_diamond(between)
        footer = self._build_straight(second_diamond, 1)
        self._build_connect(second_diamond, between)
        self._build_connect(first_diamond, header)

        self._build_connect(footer, from_)
        footer = self._build_straight(footer, 1)
        return footer

    def main(
        self, dummy_loop_count, find_loop_iterations, parallel_loops, nested_loops, base_loops
    ):
        self._construct_simple_control_flow_graph()
        self._add_dummy_loops(dummy_loop_count)
        self._construct_control_flow_graph(parallel_loops, nested_loops, base_loops)

        self._find_loops(self._loop_structure_graph)
        for _ in range(find_loop_iterations):
            self._find_loops(_LoopStructureGraph())

        self._loop_structure_graph.calculate_nesting_level()
        return [self._loop_structure_graph.get_loop_count(), self._control_flow_graph.get_node_count()]

    def _construct_control_flow_graph(self, parallel_loops, nested_loops, base_loops):
        current = 2

        for _ in range(parallel_loops):
            self._control_flow_graph.create_node(current + 1)
            self._build_connect(2, current + 1)
            current += 1

            for _ in range(nested_loops):
                top = current
                current = self._build_straight(current, 1)
                for _ in range(base_loops):
                    current = self._build_base_loop(current)
                bottom = self._build_straight(current, 1)
                self._build_connect(current, top)
                current = bottom

            self._build_connect(current, 1)

    def _add_dummy_loops(self, dummy_loop_count):
        for _ in range(dummy_loop_count):
            self._find_loops(self._loop_structure_graph)

    def _find_loops(self, loop_structure):
        finder = _HavlakLoopFinder(self._control_flow_graph, loop_structure)
        finder.find_loops()

    def _construct_simple_control_flow_graph(self):
        self._control_flow_graph.create_node(0)
        self._build_base_loop(0)
        self._control_flow_graph.create_node(1)
        _BasicBlockEdge(self._control_flow_graph, 0, 2)


_UNVISITED = 2_147_483_647
_MAX_NON_BACK_PREDECESSORS = 32 * 1024


class _BasicBlockClass(Enum):
    TOP = 0  # uninitialized
    NON_HEADER = 1  # a regular BB
    REDUCIBLE = 2  # reducible loop
    SELF_LOOP = 3  # single BB loop
    IRREDUCIBLE = 4  # irreducible loop
    DEAD = 5  # a dead BB
    LAST = 6  # Sentinel


class _HavlakLoopFinder:
    def __init__(self, control_flow_graph, loop_structure_graph):
        self._control_flow_graph = control_flow_graph
        self._loop_structure_graph = loop_structure_graph
        self._non_back_predecessors = Vector()
        self._back_predecessors = Vector()
        self._number = IdentityDictionary()
        self._max_size = 0
        self._header = None
        self._type = None
        self._last = None
        self._nodes = None

    def _is_ancestor(self, ancestor, descendant):
        return ancestor <= descendant and descendant <= self._last[ancestor]

    def _depth_first_search(self, current_node, current):
        self._nodes[current].initialize_node(current_node, current)
        self._number.at_put(current_node, current)

        last_id = current
        outer_blocks = current_node.out_edges

        def each(target):
            nonlocal last_id
            if self._number.at(target) == _UNVISITED:
                last_id = self._depth_first_search(target, last_id + 1)

        outer_blocks.for_each(each)

        self._last[current] = last_id
        return last_id

    def _initialize_all_nodes(self):
        self._control_flow_graph.basic_blocks.for_each(lambda basic_block: self._number.at_put(basic_block, _UNVISITED))

        self._depth_first_search(self._control_flow_graph.start_basic_block, 0)

    def _identify_edges(self, size):
        for candidate in range(size):
            self._header[candidate] = 0
            self._type[candidate] = _BasicBlockClass.NON_HEADER

            candidate_block = self._nodes[candidate].basic_block
            if candidate_block is None:
                self._type[candidate] = _BasicBlockClass.DEAD
            else:
                self._process_edges(candidate_block, candidate)

    def _process_edges(self, candidate_block, candidate):
        if candidate_block.get_predecessor_count() > 0:

            def each(predecessor_block):
                predecessor = self._number.at(predecessor_block)
                if predecessor != _UNVISITED:
                    if self._is_ancestor(candidate, predecessor):
                        self._back_predecessors.at(candidate).append(predecessor)
                    else:
                        self._non_back_predecessors.at(candidate).add(predecessor)

            candidate_block.in_edges.for_each(each)

    def find_loops(self):
        if self._control_flow_graph.start_basic_block is None:
            return

        size = self._control_flow_graph.get_node_count()
        self._non_back_predecessors.remove_all()
        self._back_predecessors.remove_all()
        self._number.remove_all()

        if size > self._max_size:
            self._header = [0] * size
            self._type = [None] * size
            self._last = [0] * size
            self._nodes = [None] * size
            self._max_size = size

        for index in range(size):
            self._non_back_predecessors.append(Set())
            self._back_predecessors.append(Vector())
            self._nodes[index] = _UnionFindNode()

        self._initialize_all_nodes()
        self._identify_edges(size)

        self._header[0] = 0

        for candidate in range(size - 1, -1, -1):
            node_pool = Vector()
            candidate_block = self._nodes[candidate].basic_block
            if candidate_block is not None:
                self._step_d(candidate, node_pool)

                work_list = Vector()
                node_pool.for_each(work_list.append)

                if node_pool.size() != 0:
                    self._type[candidate] = _BasicBlockClass.REDUCIBLE

                while not work_list.is_empty():
                    work_node = work_list.remove_first()

                    non_back_size = self._non_back_predecessors.at(work_node.depth_first_number).size()
                    if non_back_size > _MAX_NON_BACK_PREDECESSORS:
                        return

                    self._step_e_process_non_back_predecessors(candidate, node_pool, work_list, work_node)

            if node_pool.size() > 0 or self._type[candidate] == _BasicBlockClass.SELF_LOOP:
                loop = self._loop_structure_graph.create_new_loop(
                    candidate_block, self._type[candidate] != _BasicBlockClass.IRREDUCIBLE
                )
                self._set_loop_attributes(candidate, node_pool, loop)

    def _step_e_process_non_back_predecessors(self, candidate, node_pool, work_list, work_node):
        def each(predecessor):
            predecessor_node = self._nodes[predecessor]
            representative = predecessor_node.find_set()

            if not self._is_ancestor(candidate, representative.depth_first_number):
                self._type[candidate] = _BasicBlockClass.IRREDUCIBLE
                self._non_back_predecessors.at(candidate).add(representative.depth_first_number)
            else:
                if representative.depth_first_number != candidate:
                    if not node_pool.has_some(lambda each: each == representative):
                        work_list.append(representative)
                        node_pool.append(representative)

        self._non_back_predecessors.at(work_node.depth_first_number).for_each(each)

    def _set_loop_attributes(self, candidate, node_pool, loop):
        self._nodes[candidate].loop = loop

        def each(node):
            self._header[node.depth_first_number] = candidate
            node.union(self._nodes[candidate])

            if node.loop is not None:
                node.loop.set_parent(loop)
            else:
                loop.add_node(node.basic_block)

        node_pool.for_each(each)

    def _step_d(self, candidate, node_pool):
        def each(predecessor):
            if predecessor != candidate:
                node_pool.append(self._nodes[predecessor].find_set())
            else:
                self._type[candidate] = _BasicBlockClass.SELF_LOOP

        self._back_predecessors.at(candidate).for_each(each)
