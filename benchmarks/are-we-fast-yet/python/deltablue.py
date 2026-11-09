# Adapted for ns-next from Are We Fast Yet
# (https://github.com/smarr/are-we-fast-yet, commit
# 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Python/deltablue.py).
# Changed for ns-next: inner_benchmark_loop, benchmark and verify_result are
# bench, which runs chain_test and projection_test once at the size of
# benchmarks/are-we-fast-yet/DeltaBlue.ns, 12000; their checks are the
# benchmark's own, as in that port, and raise a RuntimeError with its messages
# ('Chain test failed!!' and 'Projection test 1 failed!!' to 'Projection test 4
# failed!!') where upstream raises an Exception with one exclamation mark;
# names are written out, as in java/deltablue/.

# This benchmark is derived from Mario Wolczko's Java and Smalltalk version of
# DeltaBlue.
#
# It is modified to use the SOM class library and Java 8 features.
# License details:
#   http://web.archive.org/web/20050825101121/
#      http://www.sunlabs.com/people/mario/java_benchmarking/index.html
from abc import abstractmethod
from enum import Enum

from benchmark import Benchmark
from som.identity_dictionary import IdentityDictionary
from som.vector import Vector, vector_with


class DeltaBlue(Benchmark):
    def bench(self):
        _Planner.chain_test(12000)
        _Planner.projection_test(12000)


class _Plan(Vector):
    def __init__(self):
        super().__init__(15)

    def execute(self):
        self.for_each(lambda constraint: constraint.execute())


class _Planner:
    def __init__(self):
        self._current_mark = 1

    # Attempt to satisfy the given constraint and, if successful,
    # incrementally update the dataflow graph. Details: If satifying
    # the constraint is successful, it may override a weaker constraint
    # on its output. The algorithm attempts to resatisfy that
    # constraint using some other method. This process is repeated
    # until either a) it reaches a variable that was not previously
    # determined by any constraint or b) it reaches a constraint that
    # is too weak to be satisfied using any of its methods. The
    # variables of constraints that have been processed are marked with
    # a unique mark value so that we know where we've been. This allows
    # the algorithm to avoid getting into an infinite loop even if the
    # constraint graph has an inadvertent cycle.
    def incremental_add(self, constraint):
        mark = self._new_mark()
        overridden = constraint.satisfy(mark, self)

        while overridden is not None:
            overridden = overridden.satisfy(mark, self)

    # Entry point for retracting a constraint. Remove the given
    # constraint and incrementally update the dataflow graph.
    # Details: Retracting the given constraint may allow some currently
    # unsatisfiable downstream constraint to be satisfied. We therefore collect
    # a list of unsatisfied downstream constraints and attempt to
    # satisfy each one in turn. This list is traversed by constraint
    # strength, strongest first, as a heuristic for avoiding
    # unnecessarily adding and then overriding weak constraints.
    # Assume: c is satisfied.
    def incremental_remove(self, constraint):
        output_variable = constraint.get_output()
        constraint.mark_unsatisfied()
        constraint.remove_from_graph()

        unsatisfied = self._remove_propagate_from(output_variable)
        unsatisfied.for_each(self.incremental_add)

    # Extract a plan for resatisfaction starting from the outputs of
    # the given constraints, usually a set of input constraints.

    def extract_plan_from_constraints(self, constraints):
        sources = Vector()

        def each(constraint):
            if constraint.is_input() and constraint.is_satisfied():
                sources.append(constraint)

        constraints.for_each(each)

        return self._make_plan(sources)

    # Extract a plan for resatisfaction starting from the given source
    # constraints, usually a set of input constraints. This method
    # assumes that stay optimization is desired; the plan will contain
    # only constraints whose output variables are not stay. Constraints
    # that do no computation, such as stay and edit constraints, are
    # not included in the plan.
    # Details: The outputs of a constraint are marked when it is added
    # to the plan under construction. A constraint may be appended to
    # the plan when all its input variables are known. A variable is
    # known if either a) the variable is marked (indicating that has
    # been computed by a constraint appearing earlier in the plan), b)
    # the variable is 'stay' (i.e. it is a constant at plan execution
    # time), or c) the variable is not determined by any
    # constraint. The last provision is for past states of history
    # variables, which are not stay but which are also not computed by
    # any constraint.
    # Assume: sources are all satisfied.
    def _make_plan(self, sources):
        mark = self._new_mark()
        plan = _Plan()
        todo = sources

        while not todo.is_empty():
            constraint = todo.remove_first()

            if constraint.get_output().mark != mark and constraint.inputs_known(mark):
                # not in plan already and eligible for inclusion
                plan.append(constraint)
                constraint.get_output().mark = mark
                self._add_constraints_consuming_to(constraint.get_output(), todo)
        return plan

    # The given variable has changed. Propagate new values downstream.
    def propagate_from(self, variable):
        todo = Vector()
        self._add_constraints_consuming_to(variable, todo)

        while not todo.is_empty():
            constraint = todo.remove_first()
            constraint.execute()
            self._add_constraints_consuming_to(constraint.get_output(), todo)

    @staticmethod
    def _add_constraints_consuming_to(variable, collection):
        determining = variable.determined_by

        def each(constraint):
            if constraint is not determining and constraint.is_satisfied():
                collection.append(constraint)

        variable.constraints.for_each(each)

    # Recompute the walkabout strengths and stay flags of all variables
    # downstream of the given constraint and recompute the actual
    # values of all variables whose stay flag is true. If a cycle is
    # detected, remove the given constraint and answer
    # false. Otherwise, answer true.
    # Details: Cycles are detected when a marked variable is
    # encountered downstream of the given constraint. The sender is
    # assumed to have marked the inputs of the given constraint with
    # the given mark. Thus, encountering a marked node downstream of
    # the output constraint means that there is a path from the
    # constraint's output to one of its inputs.
    def add_propagate(self, constraint, mark):
        todo = vector_with(constraint)

        while not todo.is_empty():
            current = todo.remove_first()

            if current.get_output().mark == mark:
                self.incremental_remove(constraint)
                return False

            current.recalculate()
            self._add_constraints_consuming_to(current.get_output(), todo)

        return True

    def change(self, variable, new_value):
        edit_constraint = _EditConstraint(variable, _PREFERRED, self)

        edit_constraints = vector_with(edit_constraint)
        plan = self.extract_plan_from_constraints(edit_constraints)
        for _ in range(10):
            variable.value = new_value
            plan.execute()

        edit_constraint.destroy_constraint(self)

    @staticmethod
    def _constraints_consuming(variable, function):
        determining = variable.determined_by

        def each(constraint):
            if constraint is not determining and constraint.is_satisfied():
                function(constraint)

        variable.constraints.for_each(each)

    # Select a previously unused mark value.
    def _new_mark(self):
        self._current_mark += 1
        return self._current_mark

    # Update the walkabout strengths and stay flags of all variables
    # downstream of the given constraint. Answer a collection of
    # unsatisfied constraints sorted in order of decreasing strength.
    def _remove_propagate_from(self, output_variable):
        unsatisfied = Vector()

        output_variable.determined_by = None
        output_variable.walk_strength = _absolute_weakest
        output_variable.stay = True

        todo = vector_with(output_variable)

        while not todo.is_empty():
            variable = todo.remove_first()

            def each(constraint):
                if not constraint.is_satisfied():
                    unsatisfied.append(constraint)

            variable.constraints.for_each(each)

            def recalculate(constraint):
                constraint.recalculate()
                todo.append(constraint.get_output())

            self._constraints_consuming(variable, recalculate)

        def compare(first, second):
            return -1 if first.strength.stronger(second.strength) else 1

        unsatisfied.sort(compare)
        return unsatisfied

    # This is the standard DeltaBlue benchmark. A long chain of
    # equality constraints is constructed with a stay constraint on
    # one end. An edit constraint is then added to the opposite end
    # and the time is measured for adding and removing this
    # constraint, and extracting and executing a constraint
    # satisfaction plan. There are two cases. In case 1, the added
    # constraint is stronger than the stay constraint and values must
    # propagate down the entire length of the chain. In case 2, the
    # added constraint is weaker than the stay constraint so it cannot
    # be accomodated. The cost in this case is, of course, very
    # low. Typical situations lie somewhere between these two
    # extremes.
    @staticmethod
    def chain_test(count):
        planner = _Planner()
        variables = [None] * (count + 1)
        for index in range(count + 1):
            variables[index] = _Variable()

        # Build chain of n equality constraints
        for index in range(count):
            first = variables[index]
            second = variables[index + 1]
            _EqualityConstraint(first, second, _REQUIRED, planner)

        _StayConstraint(variables[count], _STRONG_DEFAULT, planner)
        edit_constraint = _EditConstraint(variables[0], _PREFERRED, planner)

        edit_constraints = vector_with(edit_constraint)
        plan = planner.extract_plan_from_constraints(edit_constraints)
        for index in range(100):
            variables[0].value = index
            plan.execute()
            if variables[count].value != index:
                raise RuntimeError("Chain test failed!!")

        edit_constraint.destroy_constraint(planner)

    # This test constructs a two sets of variables related to each
    # other by a simple linear transformation (scale and offset). The
    # time is measured to change a variable on either side of the
    # mapping and to change the scale and offset factors.
    @staticmethod
    def projection_test(count):
        planner = _Planner()

        destinations = Vector()

        scale = _Variable(10)
        offset = _Variable(1000)

        source = None
        destination = None
        for index in range(1, count + 1):
            source = _Variable(index)
            destination = _Variable(index)
            destinations.append(destination)
            _StayConstraint(source, _DEFAULT, planner)
            _ScaleConstraint(source, scale, offset, destination, _REQUIRED, planner)

        planner.change(source, 17)
        if destination.value != 1170:
            raise RuntimeError("Projection test 1 failed!!")

        planner.change(destination, 1050)
        if source.value != 5:
            raise RuntimeError("Projection test 2 failed!!")

        planner.change(scale, 5)
        for index in range(count - 1):
            if destinations.at(index).value != (index + 1) * 5 + 1000:
                raise RuntimeError("Projection test 3 failed!!")

        planner.change(offset, 2000)
        for index in range(count - 1):
            if destinations.at(index).value != (index + 1) * 5 + 2000:
                raise RuntimeError("Projection test 4 failed!!")


class _Symbol:
    def __init__(self, hash_):
        self._hash = hash_

    def custom_hash(self):
        return self._hash


_ABSOLUTE_STRONGEST = _Symbol(0)
_REQUIRED = _Symbol(1)
_STRONG_PREFERRED = _Symbol(2)
_PREFERRED = _Symbol(3)
_STRONG_DEFAULT = _Symbol(4)
_DEFAULT = _Symbol(5)
_WEAK_DEFAULT = _Symbol(6)
_ABSOLUTE_WEAKEST = _Symbol(7)


class _Strength:
    def __init__(self, symbolic_value):
        self._symbolic_value = symbolic_value
        self.arithmetic_value = _strength_table.at(symbolic_value)

    def same_as(self, strength):
        return self.arithmetic_value == strength.arithmetic_value

    def stronger(self, strength):
        return self.arithmetic_value < strength.arithmetic_value

    def weaker(self, strength):
        return self.arithmetic_value > strength.arithmetic_value

    def strongest(self, strength):
        return strength if strength.stronger(self) else self

    def weakest(self, strength):
        return strength if strength.weaker(self) else self

    @staticmethod
    def of(strength):
        return _strength_constant.at(strength)


def _create_strength_table():
    strength_table = IdentityDictionary()
    strength_table.at_put(_ABSOLUTE_STRONGEST, -10000)
    strength_table.at_put(_REQUIRED, -800)
    strength_table.at_put(_STRONG_PREFERRED, -600)
    strength_table.at_put(_PREFERRED, -400)
    strength_table.at_put(_STRONG_DEFAULT, -200)
    strength_table.at_put(_DEFAULT, 0)
    strength_table.at_put(_WEAK_DEFAULT, 500)
    strength_table.at_put(_ABSOLUTE_WEAKEST, 10000)
    return strength_table


def _create_strength_constants():
    strength_constant = IdentityDictionary()
    _strength_table.get_keys().for_each(
        lambda key: strength_constant.at_put(key, _Strength(key))
    )
    return strength_constant


_strength_table = _create_strength_table()
_strength_constant = _create_strength_constants()

_absolute_weakest = _Strength.of(_ABSOLUTE_WEAKEST)
_required = _Strength.of(_REQUIRED)


class _Direction(Enum):
    FORWARD = 1
    BACKWARD = 2


# ------------------------ constraints ------------------------------------


# I am an abstract class representing a system-maintainable
# relationship (or "constraint") between a set of variables. I supply
# a strength instance variable; concrete subclasses provide a means
# of storing the constrained variables and other information required
# to represent a constraint.
class _AbstractConstraint:
    def __init__(self, strength):
        self.strength = _Strength.of(strength)

    # Normal constraints are not input constraints. An input constraint
    # is one that depends on external state, such as the mouse, the
    # keyboard, a clock, or some arbitrary piece of imperative code.
    def is_input(self):
        return False

    # Answer true if this constraint is satisfied in the current solution.
    @abstractmethod
    def is_satisfied(self):
        pass

    # Activate this constraint and attempt to satisfy it.
    def add_constraint(self, planner):
        self.add_to_graph()
        planner.incremental_add(self)

    # Add myself to the constraint graph.
    @abstractmethod
    def add_to_graph(self):
        pass

    # Deactivate this constraint, remove it from the constraint graph,
    # possibly causing other constraints to be satisfied, and destroy it.
    def destroy_constraint(self, planner):
        if self.is_satisfied():
            planner.incremental_remove(self)

        self.remove_from_graph()

    # Remove myself from the constraint graph.
    @abstractmethod
    def remove_from_graph(self):
        pass

    # Decide if I can be satisfied and record that decision. The output
    # of the chosen method must not have the given mark and must have
    # a walkabout strength less than that of this constraint.
    @abstractmethod
    def choose_method(self, mark):
        pass

    # Enforce this constraint. Assume that it is satisfied.
    @abstractmethod
    def execute(self):
        pass

    @abstractmethod
    def inputs_do(self, function):
        pass

    @abstractmethod
    def inputs_has_one(self, function):
        pass

    # Assume that I am satisfied. Answer true if all my current inputs
    # are known. A variable is known if either a) it is 'stay' (i.e. it
    # is a constant at plan execution time), b) it has the given mark
    # (indicating that it has been computed by a constraint appearing
    # earlier in the plan), or c) it is not determined by any
    # constraint.
    def inputs_known(self, mark):
        return not self.inputs_has_one(
            lambda variable: not (variable.mark == mark or variable.stay or variable.determined_by is None)
        )

    # Record the fact that I am unsatisfied.
    @abstractmethod
    def mark_unsatisfied(self):
        pass

    # Answer my current output variable. Raise an error if I am not
    # currently satisfied.
    @abstractmethod
    def get_output(self):
        pass

    # Calculate the walkabout strength, the stay flag, and, if it is
    # 'stay', the value for the current output of this
    # constraint. Assume this constraint is satisfied.
    @abstractmethod
    def recalculate(self):
        pass

    # Attempt to find a way to enforce this constraint. If successful,
    # record the solution, perhaps modifying the current dataflow
    # graph. Answer the constraint that this constraint overrides, if
    # there is one, or nil, if there isn't.
    # Assume: I am not already satisfied.
    def satisfy(self, mark, planner):
        self.choose_method(mark)

        if self.is_satisfied():
            # constraint can be satisfied
            # mark inputs to allow cycle detection in addPropagate
            def each(input_variable):
                input_variable.mark = mark

            self.inputs_do(each)

            output_variable = self.get_output()
            overridden = output_variable.determined_by
            if overridden is not None:
                overridden.mark_unsatisfied()

            output_variable.determined_by = self
            if not planner.add_propagate(self, mark):
                raise Exception("Cycle encountered")

            output_variable.mark = mark
        else:
            overridden = None
            if self.strength.same_as(_required):
                raise Exception("Could not satisfy a required constraint")

        return overridden


# I am an abstract superclass for constraints having two possible
# output variables.
class _BinaryConstraint(_AbstractConstraint):
    def __init__(self, first_variable, second_variable, strength, _planner):
        super().__init__(strength)
        self._first = first_variable
        self._second = second_variable
        self._direction = None

    # Answer true if this constraint is satisfied in the current solution.
    def is_satisfied(self):
        return self._direction is not None

    # Add myself to the constraint graph.
    def add_to_graph(self):
        self._first.add_constraint(self)
        self._second.add_constraint(self)
        self._direction = None

    # Remove myself from the constraint graph.
    def remove_from_graph(self):
        if self._first is not None:
            self._first.remove_constraint(self)
        if self._second is not None:
            self._second.remove_constraint(self)
        self._direction = None

    # Decide if I can be satisfied and which way I should flow based on
    # the relative strength of the variables I relate, and record that
    # decision.
    def choose_method(self, mark):
        if self._first.mark == mark:
            if self._second.mark != mark and self.strength.stronger(self._second.walk_strength):
                self._direction = _Direction.FORWARD
                return self._direction

            self._direction = None
            return self._direction

        if self._second.mark == mark:
            if self._first.mark != mark and self.strength.stronger(self._first.walk_strength):
                self._direction = _Direction.BACKWARD
                return self._direction

            self._direction = None
            return self._direction

        # If we get here, neither variable is marked, so we have a choice.
        if self._first.walk_strength.weaker(self._second.walk_strength):
            if self.strength.stronger(self._first.walk_strength):
                self._direction = _Direction.BACKWARD
                return self._direction

            self._direction = None
            return self._direction

        if self.strength.stronger(self._second.walk_strength):
            self._direction = _Direction.FORWARD
            return self._direction

        self._direction = None
        return self._direction

    def inputs_do(self, function):
        if self._direction is _Direction.FORWARD:
            function(self._first)
        else:
            function(self._second)

    def inputs_has_one(self, function):
        if self._direction is _Direction.FORWARD:
            return function(self._first)

        return function(self._second)

    # Record the fact that I am unsatisfied.
    def mark_unsatisfied(self):
        self._direction = None

    # Answer my current output variable.
    def get_output(self):
        return self._second if self._direction is _Direction.FORWARD else self._first

    # Calculate the walkabout strength, the stay flag, and, if it is
    # 'stay', the value for the current output of this
    # constraint. Assume this constraint is satisfied.
    def recalculate(self):
        if self._direction is _Direction.FORWARD:
            input_ = self._first
            output = self._second
        else:
            input_ = self._second
            output = self._first

        output.walk_strength = self.strength.weakest(input_.walk_strength)
        output.stay = input_.stay
        if output.stay:
            self.execute()


# I am an abstract superclass for constraints having a single
# possible output variable.
class _UnaryConstraint(_AbstractConstraint):
    def __init__(self, variable, strength, planner):
        super().__init__(strength)
        self._output = variable  # possible output variable
        self._satisfied = False  # true if I am currently satisfied
        self.add_constraint(planner)

    # Answer true if this constraint is satisfied in the current solution.
    def is_satisfied(self):
        return self._satisfied

    # Add myself to the constraint graph.
    def add_to_graph(self):
        self._output.add_constraint(self)
        self._satisfied = False

    # Remove myself from the constraint graph.
    def remove_from_graph(self):
        if self._output is not None:
            self._output.remove_constraint(self)
        self._satisfied = False

    # Decide if I can be satisfied and record that decision.
    def choose_method(self, mark):
        self._satisfied = self._output.mark != mark and self.strength.stronger(
            self._output.walk_strength
        )

    @abstractmethod
    def execute(self):
        pass

    def inputs_do(self, function):
        # I have no input variables
        pass

    def inputs_has_one(self, function):
        return False

    # Record the fact that I am unsatisfied.
    def mark_unsatisfied(self):
        self._satisfied = False

    # Answer my current output variable.
    def get_output(self):
        return self._output

    # Calculate the walkabout strength, the stay flag, and, if it is
    # 'stay', the value for the current output of this
    # constraint. Assume this constraint is satisfied."
    def recalculate(self):
        self._output.walk_strength = self.strength
        self._output.stay = not self.is_input()
        if self._output.stay:
            self.execute()  # stay optimization


# I am a unary input constraint used to mark a variable that the
# client wishes to change.
class _EditConstraint(_UnaryConstraint):
    # I indicate that a variable is to be changed by imperative code.
    def is_input(self):
        return True

    def execute(self):  # Edit constraints do nothing.
        pass


# I constrain two variables to have the same value: "v1 = v2".
class _EqualityConstraint(_BinaryConstraint):
    # Install a constraint with the given strength equating the given
    # variables.
    def __init__(self, first_variable, second_variable, strength, planner):
        super().__init__(first_variable, second_variable, strength, planner)
        self.add_constraint(planner)

    # Enforce this constraint. Assume that it is satisfied.
    def execute(self):
        if self._direction is _Direction.FORWARD:
            self._second.value = self._first.value
        else:
            self._first.value = self._second.value


# I relate two variables by the linear scaling relationship: "v2 =
# (v1 * scale) + offset". Either v1 or v2 may be changed to maintain
# this relationship but the scale factor and offset are considered
# read-only.
class _ScaleConstraint(_BinaryConstraint):
    def __init__(self, source, scale, offset, destination, strength, planner):
        super().__init__(source, destination, strength, planner)
        self._scale = scale  # scale factor input variable
        self._offset = offset  # offset input variable
        self.add_constraint(planner)

    # Add myself to the constraint graph.
    def add_to_graph(self):
        self._first.add_constraint(self)
        self._second.add_constraint(self)
        self._scale.add_constraint(self)
        self._offset.add_constraint(self)
        self._direction = None

    # Remove myself from the constraint graph.
    def remove_from_graph(self):
        if self._first is not None:
            self._first.remove_constraint(self)
        if self._second is not None:
            self._second.remove_constraint(self)
        if self._scale is not None:
            self._scale.remove_constraint(self)
        if self._offset is not None:
            self._offset.remove_constraint(self)
        self._direction = None

    # Enforce this constraint. Assume that it is satisfied.
    def execute(self):
        if self._direction is _Direction.FORWARD:
            self._second.value = self._first.value * self._scale.value + self._offset.value
        else:
            self._first.value = (self._second.value - self._offset.value) / self._scale.value

    def inputs_do(self, function):
        if self._direction is _Direction.FORWARD:
            function(self._first)
            function(self._scale)
            function(self._offset)
        else:
            function(self._second)
            function(self._scale)
            function(self._offset)

    # Calculate the walkabout strength, the stay flag, and, if it is
    # 'stay', the value for the current output of this
    # constraint. Assume this constraint is satisfied.
    def recalculate(self):
        if self._direction is _Direction.FORWARD:
            input_ = self._first
            output = self._second
        else:
            output = self._first
            input_ = self._second

        output.walk_strength = self.strength.weakest(input_.walk_strength)
        output.stay = input_.stay and self._scale.stay and self._offset.stay
        if output.stay:
            self.execute()  # stay optimization


# I mark variables that should, with some level of preference, stay
# the same. I have one method with zero inputs and one output, which
# does nothing. Planners may exploit the fact that, if I am
# satisfied, my output will not change during plan execution. This is
# called "stay optimization".
class _StayConstraint(_UnaryConstraint):
    def execute(self):
        pass  # Stay constraints do nothing.


# ------------------------------ variables ------------------------------


# I represent a constrained variable. In addition to my value, I
# maintain the structure of the constraint graph, the current
# dataflow graph, and various parameters of interest to the DeltaBlue
# incremental constraint solver.
class _Variable:
    def __init__(self, value=0):
        self.value = value  # my value; changed by constraints
        self.constraints = Vector(2)  # normal constraints that reference me
        self.determined_by = None  # the constraint that currently determines
        # my value (or null if there isn't one)
        self.mark = 0  # used by the planner to mark constraints
        self.walk_strength = _absolute_weakest  # my walkabout strength
        self.stay = True  # true if I am a planning-time constant

    # Add the given constraint to the set of all constraints that refer to me.
    def add_constraint(self, constraint):
        self.constraints.append(constraint)

    # Remove all traces of c from this variable.
    def remove_constraint(self, constraint):
        self.constraints.remove(constraint)
        if self.determined_by is constraint:
            self.determined_by = None
