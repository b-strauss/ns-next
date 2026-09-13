//! The observable outcome -- exit status, stdout and stderr, byte for byte --
//! of programs that cannot be fixtures, because they exit non-zero or pin an
//! exact message. Every expectation here is a recorded answer
//! (`docs/design/execution.md`, "One engine"), pinned so that a later change
//! cannot move it quietly. Every program runs under both lowerings, the
//! optimized one and the reference lowering it is checked against
//! (`docs/design/compiler.md`, "The reference lowering"), and each must
//! produce the recorded answer: a change that alters one of these is a
//! behaviour change and has to be justified as one.

use indoc::indoc;
mod common;

use common::ScratchDir;

/// Runs `src` with `ns-next run` under each lowering and asserts all three
/// streams exactly.
fn assert_outcome(name: &str, src: &str, exit: i32, stdout: &str, stderr: &str) {
    let dir = ScratchDir::new(name);
    let path = dir.join(&format!("{name}.ns"));
    std::fs::write(&path, src).expect("write source");
    for lowering in ["optimized", "reference"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ns-next"))
            .arg("run")
            .arg(&path)
            .arg("--lowering")
            .arg(lowering)
            .output()
            .expect("run ns-next");
        let actual_stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
        let actual_stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
        assert_eq!(
            output.status.code(),
            Some(exit),
            "{name} under the {lowering} lowering: exit status (stderr: {actual_stderr})"
        );
        assert_eq!(
            actual_stdout, stdout,
            "{name} under the {lowering} lowering: stdout"
        );
        assert_eq!(
            actual_stderr, stderr,
            "{name} under the {lowering} lowering: stderr"
        );
    }
}

#[test]
fn a_does_not_understand_error() {
    assert_outcome(
        "dnu",
        indoc! {"
            Newspeak3
            'T'
            class Dnu usingPlatform: platform = Object new (
            ) (
                public main: p args: a = (
                    3 fooBar
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: an object does not understand #fooBar\n",
    );
}

/// A block created while its class initializes, whose `^` runs after the
/// initializer has returned: its home is dead, so this is §7.2's
/// `cannotReturn`.
#[test]
fn a_return_from_a_block_whose_initializer_has_finished_is_cannot_return() {
    assert_outcome(
        "init_block_return",
        indoc! {"
            Newspeak3
            'Test'
            class InitBlockReturn usingPlatform: platform = Object new (
                | blk = [ ^2 ]. |
            ) (
                public main: p args: a = (
                    blk value.
                    p transcript show: 'unreachable'; newLine
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: cannotReturn: the enclosing method activation has already returned\n",
    );
}

/// The method-home counterpart: `stash` stores a block holding `^` and
/// returns normally; a later send invokes it after its home's frame was
/// divorced.
#[test]
fn a_return_from_a_stored_block_whose_method_has_returned_is_cannot_return() {
    assert_outcome(
        "stored_block_return",
        indoc! {"
            Newspeak3
            'Test'
            class StoredBlockReturn usingPlatform: platform = Object new (
                | blk ::= nil. |
            ) (
                stash = (
                    blk := [^'too late'].
                    ^'stashed'
                )
                public main: p args: a = (
                    p transcript show: self stash; newLine.
                    blk value.
                    p transcript show: 'unreachable'; newLine
                )
            ) : ()
        "},
        1,
        "stashed\n",
        "Uncaught Newspeak error: cannotReturn: the enclosing method activation has already returned\n",
    );
}

/// An inlined selector's guard fails on a non-Boolean receiver, and the
/// fallback is a real send: `3` does not understand `ifTrue:ifFalse:`.
#[test]
fn a_non_boolean_receiver_of_an_inlined_selector_does_not_understand() {
    assert_outcome(
        "if-dnu",
        indoc! {"
            Newspeak3
            'T'
            class IfDnu usingPlatform: platform = Object new (
            ) (
                public main: p args: a = (
                    3 ifTrue: ['a'] ifFalse: ['b']
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: an object does not understand #ifTrue:ifFalse:\n",
    );
}

/// The condition body runs exactly once, then its non-Boolean value is sent
/// `mustBeBoolean`.
#[test]
fn a_non_boolean_while_condition_fails_with_must_be_boolean() {
    assert_outcome(
        "while-must-be-boolean",
        indoc! {"
            Newspeak3
            'T'
            class WhileMbb usingPlatform: platform = Object new (
            ) (
                public main: p args: a = (
                    p transcript show: 'before'; newLine.
                    [p transcript show: 'cond'; newLine. 3] whileTrue: [nil]
                )
            ) : ()
        "},
        1,
        "before\ncond\n",
        "Uncaught Newspeak error: an object does not understand #mustBeBoolean\n",
    );
}

#[test]
fn a_variable_block_receiver_takes_the_native_loop() {
    assert_outcome(
        "while-variable",
        indoc! {"
            Newspeak3
            'T'
            class WhileVar usingPlatform: platform = Object new (
                | n ::= 0. |
            ) (
                public main: p args: a = (
                    | cond |
                    cond := [n < 3].
                    cond whileTrue: [n := n + 1].
                    p transcript show: n printString; newLine
                )
            ) : ()
        "},
        0,
        "3\n",
        "",
    );
}

/// Only the non-short-circuited arms run, and a non-Boolean block value
/// coerces to `false`.
#[test]
fn and_or_short_circuit_and_coerce() {
    assert_outcome(
        "and-or",
        indoc! {"
            Newspeak3
            'T'
            class AndOr usingPlatform: platform = Object new (
                | log ::= ''. |
            ) (
                note: s = ( log := log, s. ^true )
                public main: p args: a = (
                    p transcript show: (true and: [self note: 'A']) printString; newLine.
                    p transcript show: (false and: [self note: 'B']) printString; newLine.
                    p transcript show: (true or: [self note: 'C']) printString; newLine.
                    p transcript show: (false or: [self note: 'D']) printString; newLine.
                    p transcript show: (true and: [3]) printString; newLine.
                    p transcript show: log; newLine
                )
            ) : ()
        "},
        0,
        "true\nfalse\ntrue\ntrue\nfalse\nAD\n",
        "",
    );
}

/// A `^` inside an inlined loop body unwinds out of the whole loop, and a
/// closure built inside an inlined branch still reaches the enclosing frame.
#[test]
fn a_return_out_of_an_inlined_loop_body() {
    assert_outcome(
        "inlined-return",
        indoc! {"
            Newspeak3
            'T'
            class InlinedReturn usingPlatform: platform = Object new (
            ) (
                count: limit = (
                    | i |
                    i := 0.
                    [true] whileTrue: [
                        i := i + 1.
                        i > limit ifTrue: [^i]].
                    ^0
                )
                capture: x = (
                    | c |
                    x > 0 ifTrue: [c := [x * 2]] ifFalse: [c := [x]].
                    ^c value
                )
                public main: p args: a = (
                    p transcript show: (self count: 3) printString; newLine.
                    p transcript show: (self capture: 5) printString; newLine
                )
            ) : ()
        "},
        0,
        "4\n10\n",
        "",
    );
}

/// A send's operands mutating each other's inputs: every operand keeps the
/// value it had when it was evaluated. One case per consumer -- keyword
/// arguments, a binary send's right operand mutating the left,
/// dynamic-array elements, and an inlined arm.
#[test]
fn operands_that_mutate_each_other_keep_their_evaluated_values() {
    assert_outcome(
        "operand-order",
        indoc! {"
            Newspeak3
            'T'
            class OperandOrder usingPlatform: platform = Object new (
            ) (
                first: x second: y = ( ^x )
                keywordArgs = ( | t | t := 1. ^self first: t second: (t := 9) )
                binaryOperand = ( | t | t := 2. ^t + (t := 30) )
                arrayElements = ( | t | t := 1. ^{t. t := 5. t} at: 1 )
                inlinedArm = ( | t | t := 2. ^t + (true ifTrue: [t := 9. 1] ifFalse: [2]) )
                public main: p args: a = (
                    p transcript show: self keywordArgs printString; newLine.
                    p transcript show: self binaryOperand printString; newLine.
                    p transcript show: self arrayElements printString; newLine.
                    p transcript show: self inlinedArm printString; newLine
                )
            ) : ()
        "},
        0,
        "1\n32\n1\n3\n",
        "",
    );
}

/// The same invariant when the write happens inside a callee, through a
/// closure built in an earlier statement: a cascade receiver and an
/// assignment's value handed on through a setter send.
#[test]
fn an_operand_written_by_a_callee_keeps_its_evaluated_value() {
    assert_outcome(
        "operand-callee",
        indoc! {"
            Newspeak3
            'T'
            class CalleeClobber usingPlatform: platform = Object new (
                | c ::= nil. |
            ) (
                first: x second: y = ( ^x )
                hook: v = ( c value. ^v )
                setterValue = ( | t | t := 1. c := [t := 9]. ^self first: (hook := t) second: 1 )
                argThroughASend = ( | t | t := 1. c := [t := 9]. ^self first: t second: (self hook: 0) )
                cascadeReceiver = (
                    | t orig |
                    orig := OrderedCollection new.
                    t := orig.
                    c := [t := OrderedCollection new].
                    t add: 1; add: (self hook: 2).
                    ^orig size
                )
                public main: p args: a = (
                    p transcript show: self setterValue printString; newLine.
                    p transcript show: self argThroughASend printString; newLine.
                    p transcript show: self cascadeReceiver printString; newLine
                )
            ) : ()
        "},
        0,
        "1\n1\n2\n",
        "",
    );
}

/// `x := e` answers the value of `e` (`docs/design/dispatch.md`, "Setter
/// sends"), even when the setter it sends writes the local `e` read, through a
/// closure built earlier. One case per position that reads an assignment's
/// value after its setter send has run: a `^`, a `^` inside a block, a block's
/// last statement, the last statement of an inlined branch, of an inlined
/// `and:` block and of an inlined loop condition, and a slot initializer. Each
/// line is the value `e` had before the setter ran: `1`, `true` for the
/// `and:`, and one loop iteration, since the second test finds `false`.
#[test]
fn an_assignment_through_a_setter_answers_the_value_before_the_setter_ran() {
    assert_outcome(
        "setter-value",
        indoc! {"
            Newspeak3
            'T'
            class SetterValue usingPlatform: platform = Object new (
                | blk ::= nil. |
            ) (
                public class Init new: n = Object new (
                    | hook = [n := 99]. answer = (x := n). |
                ) (
                    x: v = ( hook value )
                    public result = ( ^answer )
                )
                x: v = ( blk value )
                returned = ( | y | y := 1. blk := [y := 99]. ^x := y )
                returnedFromABlock = ( [ | z | z := 1. blk := [z := 99]. ^x := z ] value. ^0 )
                blockValue = ( ^[ | z | z := 1. blk := [z := 99]. x := z ] value )
                inlinedBranch: c = ( | y | y := 1. blk := [y := 99]. ^c ifTrue: [x := y] ifFalse: [0] )
                inlinedAnd = ( | y | y := true. blk := [y := false]. ^true and: [x := y] )
                inlinedCondition = (
                    | y count |
                    y := true.
                    count := 0.
                    blk := [y := false].
                    [x := y] whileTrue: [count := count + 1].
                    ^count
                )
                public main: p args: a = (
                    p transcript show: self returned printString; newLine.
                    p transcript show: self returnedFromABlock printString; newLine.
                    p transcript show: self blockValue printString; newLine.
                    p transcript show: (self inlinedBranch: true) printString; newLine.
                    p transcript show: self inlinedAnd printString; newLine.
                    p transcript show: self inlinedCondition printString; newLine.
                    p transcript show: (Init new: 1) result printString; newLine
                )
            ) : ()
        "},
        0,
        "1\n1\n1\n1\ntrue\n1\n1\n",
        "",
    );
}

/// A bare `super` parses and resolves but has no binding, so it is a
/// program-level error raised where it is evaluated.
#[test]
fn a_bare_super_is_a_program_error() {
    assert_outcome(
        "bare-super",
        indoc! {"
            Newspeak3
            'T'
            class BareSuper usingPlatform: platform = Object new (
            ) (
                public main: p args: a = (
                    ^super
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: internal: unresolved identifier `super`\n",
    );
}

/// Raised where it is *evaluated*, not when the method is compiled:
/// everything before it still runs.
#[test]
fn a_bare_super_raises_only_when_it_is_reached() {
    assert_outcome(
        "bare-super-deferred",
        indoc! {"
            Newspeak3
            'T'
            class DeferredSuper usingPlatform: platform = Object new (
            ) (
                unreached = ( ^super )
                public main: p args: a = (
                    p transcript show: 'before'; newLine.
                    ^self unreached
                )
            ) : ()
        "},
        1,
        "before\n",
        "Uncaught Newspeak error: internal: unresolved identifier `super`\n",
    );
}

/// §6.1: "It is a run time error if the message determined by the
/// inheritance clause is not the primary factory message of C."
#[test]
fn a_superclass_clause_naming_the_wrong_factory_fails() {
    assert_outcome(
        "wrong-superclass-factory",
        indoc! {"
            Newspeak3
            'T'
            class WrongFactory usingPlatform: platform = Object new (
            ) (
                public class Base withBase: b = Object new (| v ::= b. |) (
                )
                public class Derived = Base new (
                ) (
                )
                public main: p args: a = (
                    ^Derived new
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: the superclass clause of class Derived sends #new, but the primary factory of class Base is #withBase:\n",
    );
}

/// `and:`'s block value coerces by `truthy`'s rule when the block is a
/// variable, which takes the native: a non-Boolean is `false`.
#[test]
fn a_variable_and_block_coerces_its_value_to_a_boolean() {
    assert_outcome(
        "and-variable-coerces",
        indoc! {"
            Newspeak3
            'T'
            class AndVar usingPlatform: platform = Object new (
            ) (
                public main: p args: a = (
                    | three yes |
                    three := [3].
                    yes := [true].
                    p transcript show: (true and: three) printString; newLine.
                    p transcript show: (true and: yes) printString; newLine.
                    p transcript show: (false or: three) printString; newLine
                )
            ) : ()
        "},
        0,
        "false\ntrue\nfalse\n",
        "",
    );
}

/// A native loop whose condition goes non-Boolean after two rounds sends
/// `mustBeBoolean` to it, after exactly two bodies.
#[test]
fn a_variable_while_condition_that_goes_non_boolean_fails_with_must_be_boolean() {
    assert_outcome(
        "while-variable-mbb",
        indoc! {"
            Newspeak3
            'T'
            class WhileVarMbb usingPlatform: platform = Object new (
                | n ::= 0. |
            ) (
                public main: p args: a = (
                    | cond body |
                    cond := [n < 2 ifTrue: [true] ifFalse: [n]].
                    body := [n := n + 1. p transcript show: n printString; newLine].
                    cond whileTrue: body
                )
            ) : ()
        "},
        1,
        "1\n2\n",
        "Uncaught Newspeak error: an object does not understand #mustBeBoolean\n",
    );
}

/// A block with a parameter passed where the native activates it with none
/// is the ordinary wrong-arity error.
#[test]
fn an_if_block_that_takes_an_argument_is_a_wrong_arity_error() {
    assert_outcome(
        "if-block-arity",
        indoc! {"
            Newspeak3
            'T'
            class IfArity usingPlatform: platform = Object new (
            ) (
                public main: p args: a = (
                    | b |
                    b := [:x | x].
                    true ifTrue: b
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: wrong number of block arguments: block takes 1, got 0\n",
    );
}

/// `KernelWeakTests.ns`'s `deepStack: 1000`, which Newspeak's own tests expect
/// to run: 1000 levels through an `ifFalse:` block. Sum of 2n for n = 1..1000.
#[test]
fn a_thousand_deep_recursion_runs() {
    assert_outcome(
        "deep-stack",
        indoc! {"
            Newspeak3
            'T'
            class Deep usingPlatform: platform = Object new (
            ) (
                deepStack: n = ( ^0 = n ifTrue: [0] ifFalse: [n + (deepStack: n - 1) + n] )
                public main: p args: a = (
                    p transcript show: (deepStack: 1000) printString; newLine
                )
            ) : ()
        "},
        0,
        "1001000\n",
        "",
    );
}

/// Unbounded recursion is an ordinary Newspeak error, not a process abort,
/// and the same one under both lowerings.
#[test]
fn an_unbounded_recursion_is_a_newspeak_error() {
    assert_outcome(
        "unbounded-recursion",
        indoc! {"
            Newspeak3
            'T'
            class Unbounded usingPlatform: platform = Object new (
            ) (
                down: n = ( ^1 + (down: n + 1) )
                public main: p args: a = (
                    p transcript show: 'start'; newLine.
                    down: 0
                )
            ) : ()
        "},
        1,
        "start\n",
        "Uncaught Newspeak error: stack overflow: recursion too deep\n",
    );
}

/// `do:` is a trampoline step, so a recursion through it is made only of
/// counted activations -- the method and the block, at every level -- and
/// ends at the activation limit like any other recursion. It used to end at a
/// second limit, on re-entries of the run loop from Rust, which no longer
/// exists (`docs/design/execution.md`, "No re-entry from Rust").
#[test]
fn a_recursion_through_do_is_bounded_by_the_activation_limit() {
    assert_outcome(
        "do-recursion",
        indoc! {"
            Newspeak3
            'T'
            class DoDeep usingPlatform: platform = Object new (
            ) (
                d: n = ( #(1) do: [:e | d: n + 1] )
                public main: p args: a = ( d: 0 )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: stack overflow: recursion too deep\n",
    );
}

/// Instantiation runs each initializer level as a trampoline step, so a class
/// whose slot initializer instantiates it again is a chain of counted
/// initializer activations and ends at the activation limit. It used to end at
/// the limit on re-entries of the run loop from Rust, which no longer exists
/// (`docs/design/execution.md`, "No re-entry from Rust").
#[test]
fn a_recursive_instantiation_is_bounded_by_the_activation_limit() {
    assert_outcome(
        "instantiation-recursion",
        indoc! {"
            Newspeak3
            'T'
            class Tower usingPlatform: platform = Object new (
            ) (
                public class Floor = Object new (
                    | above = Floor new. |
                ) (
                )
                public main: p args: a = ( Floor new )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: stack overflow: recursion too deep\n",
    );
}

/// The nested-class accessor runs a superclass clause as a trampoline step,
/// the first time a class is reached on each enclosing object. `Next`'s
/// clause, `self deeper`, reaches `Next` again on a fresh `Link`, so every
/// level is a new enclosing object -- no cyclic-definition error -- and two
/// more counted activations, the clause and `deeper`, until the activation
/// limit. `Link` has no slots, so its instantiation activates nothing. It used
/// to end at the limit on re-entries of the run loop from Rust, which no
/// longer exists (`docs/design/execution.md`, "No re-entry from Rust").
#[test]
fn a_recursion_through_the_nested_class_accessor_is_bounded_by_the_activation_limit() {
    assert_outcome(
        "accessor-recursion",
        indoc! {"
            Newspeak3
            'T'
            class Chain usingPlatform: platform = Object new (
            ) (
                public class Link = Object new (
                ) (
                    public class Next = self deeper (
                    ) (
                    )
                    deeper = ( ^Link new Next )
                )
                public main: p args: a = ( Link new Next )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: stack overflow: recursion too deep\n",
    );
}
