# Translated for ns-next from `DeltaBlueTon80.ns`, itself adapted from
# Primordial Soup's `DeltaBlue.ns`, commit
# `338f0bb342a618f5608ab72ab1b1fae9b226e472`: a one-way constraint solver,
# originally written in Smalltalk by John Maloney and Mario Wolczko. It is
# not the Are We Fast Yet DeltaBlue, which is deltablue.py beside it. The
# module's slots are the module-private constants below and the planner of
# `DeltaBlueTon80`, which, as in the port, serves every run of one instance.
# The port's List is som.Vector, its `add:` append and `at: index + 1`
# at(index), the same element of a list counted from 0; its constraints
# reach their enclosing Planner lexically, here as a constructor argument;
# its Errors are RuntimeErrors with their messages.

# Copyright 2008, the Newspeak project authors.
#
# Licensed under the Apache License, Version 2.0 (the ''License''); you
# may not use this file except in compliance with the License. You may
# obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
# and in THIRD-PARTY-NOTICES.md at the repository root.
from abc import abstractmethod

from benchmark import Benchmark
from som.vector import Vector


class _Strength:
    """Strengths are used to measure the relative importance of constraints.

    New strengths may be inserted in the strength hierarchy without
    disrupting current constraints. Strengths cannot be created outside this
    module, so identity can be used for value comparison.
    """

    def __init__(self, name, value):
        self.name = name
        self.value = value

    def stronger_than(self, other):
        return self.value < other.value

    def strongest(self, other):
        return self if self.stronger_than(other) else other

    def weaker_than(self, other):
        return self.value > other.value

    def weakest(self, other):
        return self if self.weaker_than(other) else other


class _Direction:
    def __init__(self, name):
        self.name = name


_REQUIRED = _Strength("required", 0)
_STRONG_PREFERRED = _Strength("strongPreferred", 1)
_PREFERRED = _Strength("preferred", 2)
_STRONG_DEFAULT = _Strength("strongDefault", 3)
_NORMAL = _Strength("normal", 4)
_WEAK_DEFAULT = _Strength("weakDefault", 5)
_WEAKEST = _Strength("weakest", 6)
_DESCENDING_STRENGTHS = [
    _REQUIRED,
    _STRONG_PREFERRED,
    _PREFERRED,
    _STRONG_DEFAULT,
    _NORMAL,
    _WEAK_DEFAULT,
    _WEAKEST,
]
_FORWARD = _Direction("forward")
_BACKWARD = _Direction("backward")


class _Plan:
    """A Plan is an ordered list of constraints to be executed in sequence to
    resatisfy all currently satisfiable constraints in the face of one or more
    changing inputs."""

    def __init__(self):
        self._constraints = Vector()

    def add_constraint(self, constraint):
        self._constraints.append(constraint)

    def execute(self):
        """Execute my constraints in order."""
        self._constraints.for_each(lambda constraint: constraint.execute())


class _Variable:
    """I represent a constrained variable.

    In addition to my value, I maintain the structure of the constraint
    graph, the current dataflow graph, and various parameters of interest to
    the DeltaBlue incremental constraint solver.
    """

    def __init__(self, name, initial_value):
        self.value = initial_value
        self.constraints = Vector(2)
        self.determined_by = None
        self.mark = 0
        self.walk_strength = _WEAKEST
        self.stay = True
        self.name = name

    def add_constraint(self, constraint):
        """Add the given constraint to the set of all constraints that refer
        to me."""
        self.constraints.append(constraint)

    def remove_constraint(self, constraint):
        """Remove all traces of the given constraint from this variable."""
        self.constraints.remove(constraint)
        if self.determined_by is constraint:
            self.determined_by = None


class _Constraint:
    """I am an abstract class representing a system-maintainable relationship
    (or "constraint") between a set of variables.

    I supply a strength instance variable; concrete subclasses provide a means
    of storing the constrained variables and other information required to
    represent a constraint. Every concrete constraint has an `output`, its
    current output variable.
    """

    def __init__(self, planner, strength):
        self._planner = planner
        self.strength = strength

    def add_constraint(self):
        """Activate this constraint and attempt to satisfy it."""
        self.add_to_graph()
        self._planner.incremental_add(self)

    @abstractmethod
    def add_to_graph(self):
        """Add myself to the constraint graph."""

    @abstractmethod
    def choose_method(self, mark):
        """Decide if I can be satisfied and record that decision.

        The output of the chosen method must not have the given mark and must
        have a walkabout strength less than that of this constraint.
        """

    def destroy_constraint(self):
        """Deactivate this constraint, remove it from the constraint graph,
        possibly causing other constraints to be satisfied, and destroy it."""
        if self.is_satisfied():
            self._planner.incremental_remove(self)
        self.remove_from_graph()

    @abstractmethod
    def execute(self):
        """Enforce this constraint. Assume that it is satisfied."""

    @abstractmethod
    def inputs_known(self, mark):
        """Assume that I am satisfied. Answer true if all my current inputs
        are known.

        A variable is known if either a) it is 'stay' (i.e. it is a constant
        at plan execution time), b) it has the given mark (indicating that it
        has been computed by a constraint appearing earlier in the plan), or
        c) it is not determined by any constraint.
        """

    def is_input(self):
        """Normal constraints are not input constraints.

        An input constraint is one that depends on external state, such as
        the mouse, the keyboard, a clock, or some arbitrary piece of
        imperative code.
        """
        return False

    @abstractmethod
    def is_satisfied(self):
        """Answer true if this constraint is satisfied in the current
        solution."""

    @abstractmethod
    def mark_inputs(self, mark):
        """Set the mark of all input from the given mark."""

    @abstractmethod
    def mark_unsatisfied(self):
        """Record the fact that I am unsatisfied."""

    @abstractmethod
    def recalculate(self):
        """Calculate the walkabout strength, the stay flag, and, if it is
        'stay', the value for the current output of this constraint. Assume
        this constraint is satisfied."""

    @abstractmethod
    def remove_from_graph(self):
        """Remove myself from the constraint graph."""

    def satisfy(self, mark):
        """Attempt to find a way to enforce this constraint.

        If successful, record the solution, perhaps modifying the current
        dataflow graph. Answer the constraint that this constraint overrides,
        if there is one, or None, if there isn't. Assume: I am not already
        satisfied.
        """
        self.choose_method(mark)
        if not self.is_satisfied():
            if self.strength is _REQUIRED:
                raise RuntimeError("Could not satisfy a required constraint")
            return None
        # constraint can be satisfied
        #
        # mark inputs to allow cycle detection in add_propagate
        self.mark_inputs(mark)
        output_variable = self.output
        overridden = output_variable.determined_by
        if overridden is not None:
            overridden.mark_unsatisfied()
        output_variable.determined_by = self
        if not self._planner.add_propagate(self, mark):
            raise RuntimeError("Cycle encountered")
        output_variable.mark = mark
        return overridden


class _UnaryConstraint(_Constraint):
    """I am an abstract superclass for constraints having a single possible
    output variable."""

    def __init__(self, planner, variable, strength):
        super().__init__(planner, strength)
        self.output = variable
        self._satisfied = False

    def add_to_graph(self):
        """Add myself to the constraint graph."""
        self.output.add_constraint(self)
        self._satisfied = False

    def choose_method(self, mark):
        """Decide if I can be satisfied and record that decision."""
        self._satisfied = self.output.mark != mark and self.strength.stronger_than(
            self.output.walk_strength
        )

    def inputs_known(self, mark):
        return True

    def is_satisfied(self):
        """Answer true if this constraint is satisfied in the current
        solution."""
        return self._satisfied

    def mark_inputs(self, mark):
        """I have no inputs."""

    def mark_unsatisfied(self):
        """Record the fact that I am unsatisfied."""
        self._satisfied = False

    def recalculate(self):
        """Calculate the walkabout strength, the stay flag, and, if it is
        'stay', the value for the current output of this constraint. Assume
        this constraint is satisfied."""
        self.output.walk_strength = self.strength
        self.output.stay = not self.is_input()
        if self.output.stay:
            self.execute()  # Stay optimization

    def remove_from_graph(self):
        """Remove myself from the constraint graph."""
        if self.output is not None:
            self.output.remove_constraint(self)
        self._satisfied = False


class _EditConstraint(_UnaryConstraint):
    """I am a unary input constraint used to mark a variable that the client
    wishes to change."""

    def __init__(self, planner, variable, strength):
        super().__init__(planner, variable, strength)
        self.add_constraint()

    def execute(self):
        """Edit constraints do nothing."""

    def is_input(self):
        """I indicate that a variable is to be changed by imperative code."""
        return True


class _StayConstraint(_UnaryConstraint):
    """I mark variables that should, with some level of preference, stay the
    same.

    I have one method with zero inputs and one output, which does nothing.
    Planners may exploit the fact that, if I am satisfied, my output will not
    change during plan execution. This is called "stay optimization".
    """

    def __init__(self, planner, variable, strength):
        super().__init__(planner, variable, strength)
        self.add_constraint()

    def execute(self):
        """Stay constraints do nothing."""


class _BinaryConstraint(_Constraint):
    """I am an abstract superclass for constraints having two possible output
    variables."""

    def __init__(self, planner, first, second, strength):
        super().__init__(planner, strength)
        self._first = first
        self._second = second
        self._direction = None

    def add_to_graph(self):
        """Add myself to the constraint graph."""
        self._first.add_constraint(self)
        self._second.add_constraint(self)
        self._direction = None

    def choose_method(self, mark):
        """Decide if I can be satisfied and which way I should flow based on
        the relative strength of the variables I relate, and record that
        decision."""
        if self._first.mark == mark:
            if self._second.mark != mark and self.strength.stronger_than(
                self._second.walk_strength
            ):
                self._direction = _FORWARD
            else:
                self._direction = None
            return

        if self._second.mark == mark:
            if self._first.mark != mark and self.strength.stronger_than(
                self._first.walk_strength
            ):
                self._direction = _BACKWARD
            else:
                self._direction = None
            return

        # If we get here, neither variable is marked, so we have a choice.
        if self._first.walk_strength.weaker_than(self._second.walk_strength):
            if self.strength.stronger_than(self._first.walk_strength):
                self._direction = _BACKWARD
            else:
                self._direction = None
        elif self.strength.stronger_than(self._second.walk_strength):
            self._direction = _FORWARD
        else:
            # nil
            self._direction = _BACKWARD

    @property
    def input(self):
        """My current input variable."""
        return self._first if self._direction is _FORWARD else self._second

    def inputs_known(self, mark):
        input_variable = self.input
        return (
            input_variable.mark == mark
            or input_variable.stay
            or input_variable.determined_by is None
        )

    def is_satisfied(self):
        """Answer true if this constraint is satisfied in the current
        solution."""
        return self._direction is not None

    def mark_inputs(self, mark):
        """Mark the input variable with the given mark."""
        self.input.mark = mark

    def mark_unsatisfied(self):
        """Record the fact that I am unsatisfied."""
        self._direction = None

    @property
    def output(self):
        """My current output variable."""
        return self._second if self._direction is _FORWARD else self._first

    def recalculate(self):
        """Calculate the walkabout strength, the stay flag, and, if it is
        'stay', the value for the current output of this constraint. Assume
        this constraint is satisfied."""
        input_variable = self.input
        output_variable = self.output
        output_variable.walk_strength = self.strength.weakest(
            input_variable.walk_strength
        )
        output_variable.stay = input_variable.stay
        if output_variable.stay:
            self.execute()

    def remove_from_graph(self):
        """Remove myself from the constraint graph."""
        if self._first is not None:
            self._first.remove_constraint(self)
        if self._second is not None:
            self._second.remove_constraint(self)
        self._direction = None


class _EqualityConstraint(_BinaryConstraint):
    """I constrain two variables to have the same value: "first = second"."""

    def __init__(self, planner, first, second, strength):
        super().__init__(planner, first, second, strength)
        self.add_constraint()

    def execute(self):
        """Enforce this constraint. Assume that it is satisfied."""
        self.output.value = self.input.value


class _ScaleConstraint(_BinaryConstraint):
    """I relate two variables by the linear scaling relationship:
    "second = (first * scale) + offset".

    Either first or second may be changed to maintain this relationship but
    the scale factor and offset are considered read-only.
    """

    def __init__(self, planner, source, scale, offset, destination, strength):
        super().__init__(planner, source, destination, strength)
        self._scale = scale  # scale factor input variable
        self._offset = offset  # offset input variable
        self.add_constraint()

    def add_to_graph(self):
        """Add myself to the constraint graph."""
        super().add_to_graph()
        self._scale.add_constraint(self)
        self._offset.add_constraint(self)

    def execute(self):
        """Enforce this constraint. Assume that it is satisfied."""
        if self._direction is _FORWARD:
            self._second.value = (
                self._first.value * self._scale.value + self._offset.value
            )
        else:
            self._first.value = (
                self._second.value - self._offset.value
            ) // self._scale.value

    def mark_inputs(self, mark):
        """Mark the inputs from the given mark."""
        super().mark_inputs(mark)
        self._scale.mark = mark
        self._offset.mark = mark

    def recalculate(self):
        """Calculate the walkabout strength, the stay flag, and, if it is
        'stay', the value for the current output of this constraint. Assume
        this constraint is satisfied."""
        input_variable = self.input
        output_variable = self.output
        output_variable.walk_strength = self.strength.weakest(
            input_variable.walk_strength
        )
        output_variable.stay = (
            input_variable.stay and self._scale.stay and self._offset.stay
        )
        if output_variable.stay:
            self.execute()  # stay optimization

    def remove_from_graph(self):
        """Remove myself from the constraint graph."""
        super().remove_from_graph()
        if self._scale is not None:
            self._scale.remove_constraint(self)
        if self._offset is not None:
            self._offset.remove_constraint(self)


class _Planner:
    """I embody the DeltaBlue algorithm described in: ''The DeltaBlue
    Algorithm: An Incremental Constraint Hierarchy Solver'' by Bjorn N.
    Freeman-Benson and John Maloney. See January 1990 Communications of the
    ACM or University of Washington TR 89-08-06 for further details."""

    def __init__(self):
        self._current_mark = 0

    @staticmethod
    def add_constraints_consuming(variable, todo):
        determining = variable.determined_by

        def add_if_consuming(constraint):
            if constraint is not determining and constraint.is_satisfied():
                todo.append(constraint)

        variable.constraints.for_each(add_if_consuming)

    def add_propagate(self, constraint, mark):
        """Recompute the walkabout strengths and stay flags of all variables
        downstream of the given constraint and recompute the actual values of
        all variables whose stay flag is true. If a cycle is detected, remove
        the given constraint and answer False. Otherwise, answer True."""
        # Details: Cycles are detected when a marked variable is encountered
        # downstream of the given constraint. The sender is assumed to have
        # marked the inputs of the given constraint with the given mark.
        # Thus, encountering a marked node downstream of the output
        # constraint means that there is a path from the constraint's output
        # to one of its inputs.
        todo = Vector()
        todo.append(constraint)
        while not todo.is_empty():
            current = todo.remove_last()
            if current.output.mark == mark:
                self.incremental_remove(constraint)
                return False
            current.recalculate()
            self.add_constraints_consuming(current.output, todo)
        return True

    def chain_test(self, count):
        """This is the standard DeltaBlue benchmark.

        A long chain of equality constraints is constructed with a stay
        constraint on one end. An edit constraint is then added to the
        opposite end and the time is measured for adding and removing this
        constraint, and extracting and executing a constraint satisfaction
        plan. There are two cases. In case 1, the added constraint is
        stronger than the stay constraint and values must propagate down the
        entire length of the chain. In case 2, the added constraint is weaker
        than the stay constraint so it cannot be accommodated. The cost in
        this case is, of course, very low. Typical situations lie somewhere
        between these two extremes.
        """
        previous = None
        first = None
        last = None
        for index in range(1, count + 1):
            name = f"v{index}"
            variable = _Variable(name, 0)
            if previous is not None:
                _EqualityConstraint(self, previous, variable, _REQUIRED)
            if index == 1:
                first = variable
            if index == count:
                last = variable
            previous = variable

        _StayConstraint(self, last, _STRONG_DEFAULT)
        edit_constraint = _EditConstraint(self, first, _PREFERRED)
        edit_constraints = Vector()
        edit_constraints.append(edit_constraint)
        plan = self.extract_plan_from_constraints(edit_constraints)
        for index in range(1, count + 1):
            first.value = index
            plan.execute()
            if last.value != index:
                raise RuntimeError("Chain test failed!")
        edit_constraint.destroy_constraint()

    def extract_plan_from_constraints(self, constraints):
        """Extract a plan for resatisfaction starting from the outputs of the
        given constraints, usually a set of input constraints."""
        sources = Vector()

        def add_if_satisfied_input(constraint):
            if constraint.is_input() and constraint.is_satisfied():
                sources.append(constraint)

        constraints.for_each(add_if_satisfied_input)
        return self.make_plan(sources)

    def incremental_add(self, constraint):
        """Attempt to satisfy the given constraint and, if successful,
        incrementally update the dataflow graph.

        Details: If satisfying the constraint is successful, it may override a
        weaker constraint on its output. The algorithm attempts to resatisfy
        that constraint using some other method. This process is repeated
        until either a) it reaches a variable that was not previously
        determined by any constraint or b) it reaches a constraint that is too
        weak to be satisfied using any of its methods. The variables of
        constraints that have been processed are marked with a unique mark
        value so that we know where we've been. This allows the algorithm to
        avoid getting into an infinite loop even if the constraint graph has
        an inadvertent cycle.
        """
        mark = self.new_mark()
        overridden = constraint.satisfy(mark)
        while overridden is not None:
            overridden = overridden.satisfy(mark)

    def incremental_remove(self, constraint):
        """Entry point for retracting a constraint. Remove the given
        constraint and incrementally update the dataflow graph."""
        # Details: Retracting the given constraint may allow some currently
        # unsatisfiable downstream constraint to be satisfied. We therefore
        # collect a list of unsatisfied downstream constraints and attempt to
        # satisfy each one in turn. This list is traversed by constraint
        # strength, strongest first, as a heuristic for avoiding
        # unnecessarily adding and then overriding weak constraints. Assume:
        # the given constraint is satisfied.
        output_variable = constraint.output
        constraint.mark_unsatisfied()
        constraint.remove_from_graph()
        unsatisfied = self.remove_propagate_from(output_variable)
        for strength in _DESCENDING_STRENGTHS:

            def add_if_of_strength(each):
                if each.strength is strength:
                    self.incremental_add(each)

            unsatisfied.for_each(add_if_of_strength)

    def make_plan(self, sources):
        """Extract a plan for resatisfaction starting from the given source
        constraints, usually a set of input constraints.

        This method assumes that stay optimization is desired; the plan will
        contain only constraints whose output variables are not stay.
        Constraints that do no computation, such as stay and edit
        constraints, are not included in the plan.
        """
        # Details: The outputs of a constraint are marked when it is added to
        # the plan under construction. A constraint may be appended to the
        # plan when all its input variables are known. A variable is known if
        # either a) the variable is marked (indicating that has been computed
        # by a constraint appearing earlier in the plan), b) the variable is
        # 'stay' (i.e. it is a constant at plan execution time), or c) the
        # variable is not determined by any constraint. The last provision is
        # for past states of history variables, which are not stay but which
        # are also not computed by any constraint. Assume: sources are all
        # satisfied.
        mark = self.new_mark()
        plan = _Plan()
        todo = sources
        while not todo.is_empty():
            constraint = todo.remove_last()
            if constraint.output.mark != mark and constraint.inputs_known(mark):
                # not in plan already and eligible for inclusion
                plan.add_constraint(constraint)
                constraint.output.mark = mark
                self.add_constraints_consuming(constraint.output, todo)
        return plan

    def new_mark(self):
        """Select a previously unused mark value."""
        self._current_mark += 1
        return self._current_mark

    def projection_test(self, count):
        """This test constructs a two sets of variables related to each other
        by a simple linear transformation (scale and offset).

        The time is measured to change a variable on either side of the
        mapping and to change the scale and offset factors.
        """
        scale = _Variable("scale", 10)
        offset = _Variable("offset", 1000)
        source = None
        destination = None
        destinations = Vector()
        for index in range(count):
            source = _Variable(f"src{index}", index)
            destination = _Variable(f"dst{index}", index)
            destinations.append(destination)
            _StayConstraint(self, source, _NORMAL)
            _ScaleConstraint(self, source, scale, offset, destination, _REQUIRED)

        self.set_value_of(source, 17)
        if destination.value != 1170:
            raise RuntimeError("Projection test 1 failed!")
        self.set_value_of(destination, 1050)
        if source.value != 5:
            raise RuntimeError("Projection test 2 failed!")
        self.set_value_of(scale, 5)
        for index in range(count - 1):
            if destinations.at(index).value != index * 5 + 1000:
                raise RuntimeError("Projection test 3 failed!")
        self.set_value_of(offset, 2000)
        for index in range(count - 1):
            if destinations.at(index).value != index * 5 + 2000:
                raise RuntimeError("Projection test 4 failed!")

    def propagate_from(self, variable):
        """The given variable has changed. Propagate new values downstream."""
        todo = Vector()
        self.add_constraints_consuming(variable, todo)
        while not todo.is_empty():
            constraint = todo.remove_last()
            constraint.execute()
            self.add_constraints_consuming(constraint.output, todo)

    def remove_propagate_from(self, output_variable):
        """Update the walkabout strengths and stay flags of all variables
        downstream of the given constraint. Answer a collection of
        unsatisfied constraints sorted in order of decreasing strength."""
        output_variable.determined_by = None
        output_variable.walk_strength = _WEAKEST
        output_variable.stay = True
        unsatisfied = Vector()
        todo = Vector()
        todo.append(output_variable)
        while not todo.is_empty():
            variable = todo.remove_last()

            def add_if_unsatisfied(constraint):
                if not constraint.is_satisfied():
                    unsatisfied.append(constraint)

            variable.constraints.for_each(add_if_unsatisfied)
            determining = variable.determined_by

            def recalculate_if_downstream(constraint):
                if constraint is not determining and constraint.is_satisfied():
                    constraint.recalculate()
                    todo.append(constraint.output)

            variable.constraints.for_each(recalculate_if_downstream)
        return unsatisfied

    def set_value_of(self, variable, new_value):
        edit_constraint = _EditConstraint(self, variable, _PREFERRED)
        edit_constraints = Vector()
        edit_constraints.append(edit_constraint)
        plan = self.extract_plan_from_constraints(edit_constraints)
        for _ in range(10):
            variable.value = new_value
            plan.execute()
        edit_constraint.destroy_constraint()


class DeltaBlueTon80(Benchmark):
    def __init__(self):
        self._planner = _Planner()

    def benchmark(self):
        self._planner.chain_test(100)
        self._planner.projection_test(100)
        return True

    def verify_result(self, result):
        return result
