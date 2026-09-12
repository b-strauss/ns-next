//! The observable outcome -- exit status, stdout and stderr, byte for byte --
//! of programs that cannot be fixtures, because they exit non-zero or pin an
//! exact message. Every expectation here is a recorded answer
//! (`docs/design/execution.md`, "One engine"), pinned so that a later change
//! cannot move it quietly. Every program runs under both lowerings, the
//! optimized one and the reference lowering it is checked against
//! (`docs/design/compiler.md`, "The reference lowering"), and each must
//! produce the recorded answer: a change that alters one of these is a
//! behaviour change and has to be justified as one.

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
        "Newspeak3\n'T'\nclass Dnu usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t3 fooBar\n\t)\n) : ()\n",
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
        "Newspeak3\n'Test'\nclass InitBlockReturn usingPlatform: platform = Object new (\n\
         \t| blk = [ ^2 ]. |\n) (\n\
         \tpublic main: p args: a = (\n\
         \t\tblk value.\n\
         \t\tp transcript show: 'unreachable'; newLine\n\
         \t)\n) : ()\n",
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
        "Newspeak3\n'Test'\nclass StoredBlockReturn usingPlatform: platform = Object new (\n\
         \t| blk ::= nil. |\n) (\n\
         \tstash = (\n\
         \t\tblk := [^'too late'].\n\
         \t\t^'stashed'\n\
         \t)\n\
         \tpublic main: p args: a = (\n\
         \t\tp transcript show: self stash; newLine.\n\
         \t\tblk value.\n\
         \t\tp transcript show: 'unreachable'; newLine\n\
         \t)\n) : ()\n",
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
        "Newspeak3\n'T'\nclass IfDnu usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t3 ifTrue: ['a'] ifFalse: ['b']\n\t)\n) : ()\n",
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
        "Newspeak3\n'T'\nclass WhileMbb usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\tp transcript show: 'before'; newLine.\n\t\t[p transcript show: 'cond'; newLine. 3] whileTrue: [nil]\n\t)\n) : ()\n",
        1,
        "before\ncond\n",
        "Uncaught Newspeak error: an object does not understand #mustBeBoolean\n",
    );
}

#[test]
fn a_variable_block_receiver_takes_the_native_loop() {
    assert_outcome(
        "while-variable",
        "Newspeak3\n'T'\nclass WhileVar usingPlatform: platform = Object new (\n\t| n ::= 0. |\n) (\n\tpublic main: p args: a = (\n\t\t| cond |\n\t\tcond := [n < 3].\n\t\tcond whileTrue: [n := n + 1].\n\t\tp transcript show: n printString; newLine\n\t)\n) : ()\n",
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
        "Newspeak3\n'T'\nclass AndOr usingPlatform: platform = Object new (\n\t| log ::= ''. |\n) (\n\tnote: s = ( log := log, s. ^true )\n\tpublic main: p args: a = (\n\t\tp transcript show: (true and: [self note: 'A']) printString; newLine.\n\t\tp transcript show: (false and: [self note: 'B']) printString; newLine.\n\t\tp transcript show: (true or: [self note: 'C']) printString; newLine.\n\t\tp transcript show: (false or: [self note: 'D']) printString; newLine.\n\t\tp transcript show: (true and: [3]) printString; newLine.\n\t\tp transcript show: log; newLine\n\t)\n) : ()\n",
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
        "Newspeak3\n'T'\nclass InlinedReturn usingPlatform: platform = Object new (\n) (\n\tcount: limit = (\n\t\t| i |\n\t\ti := 0.\n\t\t[true] whileTrue: [\n\t\t\ti := i + 1.\n\t\t\ti > limit ifTrue: [^i]].\n\t\t^0\n\t)\n\tcapture: x = (\n\t\t| c |\n\t\tx > 0 ifTrue: [c := [x * 2]] ifFalse: [c := [x]].\n\t\t^c value\n\t)\n\tpublic main: p args: a = (\n\t\tp transcript show: (self count: 3) printString; newLine.\n\t\tp transcript show: (self capture: 5) printString; newLine\n\t)\n) : ()\n",
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
        "Newspeak3\n'T'\nclass OperandOrder usingPlatform: platform = Object new (\n) (\n\tfirst: x second: y = ( ^x )\n\tkeywordArgs = ( | t | t := 1. ^self first: t second: (t := 9) )\n\tbinaryOperand = ( | t | t := 2. ^t + (t := 30) )\n\tarrayElements = ( | t | t := 1. ^{t. t := 5. t} at: 1 )\n\tinlinedArm = ( | t | t := 2. ^t + (true ifTrue: [t := 9. 1] ifFalse: [2]) )\n\tpublic main: p args: a = (\n\t\tp transcript show: self keywordArgs printString; newLine.\n\t\tp transcript show: self binaryOperand printString; newLine.\n\t\tp transcript show: self arrayElements printString; newLine.\n\t\tp transcript show: self inlinedArm printString; newLine\n\t)\n) : ()\n",
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
        "Newspeak3\n'T'\nclass CalleeClobber usingPlatform: platform = Object new (\n\t| c ::= nil. |\n) (\n\tfirst: x second: y = ( ^x )\n\thook: v = ( c value. ^v )\n\tsetterValue = ( | t | t := 1. c := [t := 9]. ^self first: (hook := t) second: 1 )\n\targThroughASend = ( | t | t := 1. c := [t := 9]. ^self first: t second: (self hook: 0) )\n\tcascadeReceiver = (\n\t\t| t orig |\n\t\torig := OrderedCollection new.\n\t\tt := orig.\n\t\tc := [t := OrderedCollection new].\n\t\tt add: 1; add: (self hook: 2).\n\t\t^orig size\n\t)\n\tpublic main: p args: a = (\n\t\tp transcript show: self setterValue printString; newLine.\n\t\tp transcript show: self argThroughASend printString; newLine.\n\t\tp transcript show: self cascadeReceiver printString; newLine\n\t)\n) : ()\n",
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
        "Newspeak3\n'T'\nclass SetterValue usingPlatform: platform = Object new (\n\
         \t| blk ::= nil. |\n) (\n\
         \tpublic class Init new: n = Object new (\n\
         \t\t| hook = [n := 99]. answer = (x := n). |\n\
         \t) (\n\
         \t\tx: v = ( hook value )\n\
         \t\tpublic result = ( ^answer )\n\
         \t)\n\
         \tx: v = ( blk value )\n\
         \treturned = ( | y | y := 1. blk := [y := 99]. ^x := y )\n\
         \treturnedFromABlock = ( [ | z | z := 1. blk := [z := 99]. ^x := z ] value. ^0 )\n\
         \tblockValue = ( ^[ | z | z := 1. blk := [z := 99]. x := z ] value )\n\
         \tinlinedBranch: c = ( | y | y := 1. blk := [y := 99]. ^c ifTrue: [x := y] ifFalse: [0] )\n\
         \tinlinedAnd = ( | y | y := true. blk := [y := false]. ^true and: [x := y] )\n\
         \tinlinedCondition = (\n\
         \t\t| y count |\n\
         \t\ty := true.\n\
         \t\tcount := 0.\n\
         \t\tblk := [y := false].\n\
         \t\t[x := y] whileTrue: [count := count + 1].\n\
         \t\t^count\n\
         \t)\n\
         \tpublic main: p args: a = (\n\
         \t\tp transcript show: self returned printString; newLine.\n\
         \t\tp transcript show: self returnedFromABlock printString; newLine.\n\
         \t\tp transcript show: self blockValue printString; newLine.\n\
         \t\tp transcript show: (self inlinedBranch: true) printString; newLine.\n\
         \t\tp transcript show: self inlinedAnd printString; newLine.\n\
         \t\tp transcript show: self inlinedCondition printString; newLine.\n\
         \t\tp transcript show: (Init new: 1) result printString; newLine\n\
         \t)\n) : ()\n",
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
        "Newspeak3\n'T'\nclass BareSuper usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t^super\n\t)\n) : ()\n",
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
        "Newspeak3\n'T'\nclass DeferredSuper usingPlatform: platform = Object new (\n) (\n\tunreached = ( ^super )\n\tpublic main: p args: a = (\n\t\tp transcript show: 'before'; newLine.\n\t\t^self unreached\n\t)\n) : ()\n",
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
        "Newspeak3\n'T'\nclass WrongFactory usingPlatform: platform = Object new (\n) (\n\tpublic class Base withBase: b = Object new (| v ::= b. |) (\n\t)\n\tpublic class Derived = Base new (\n\t) (\n\t)\n\tpublic main: p args: a = (\n\t\t^Derived new\n\t)\n) : ()\n",
        1,
        "",
        "Uncaught Newspeak error: the superclass clause of class Derived sends #new, but the primary factory of class Base is #withBase:\n",
    );
}

/// `and:`'s block value coerces by `truthy_arg`'s rule when the block is a
/// variable, which takes the native: a non-Boolean is `false`.
#[test]
fn a_variable_and_block_coerces_its_value_to_a_boolean() {
    assert_outcome(
        "and-variable-coerces",
        "Newspeak3\n'T'\nclass AndVar usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t| three yes |\n\t\tthree := [3].\n\t\tyes := [true].\n\t\tp transcript show: (true and: three) printString; newLine.\n\t\tp transcript show: (true and: yes) printString; newLine.\n\t\tp transcript show: (false or: three) printString; newLine\n\t)\n) : ()\n",
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
        "Newspeak3\n'T'\nclass WhileVarMbb usingPlatform: platform = Object new (\n\t| n ::= 0. |\n) (\n\tpublic main: p args: a = (\n\t\t| cond body |\n\t\tcond := [n < 2 ifTrue: [true] ifFalse: [n]].\n\t\tbody := [n := n + 1. p transcript show: n printString; newLine].\n\t\tcond whileTrue: body\n\t)\n) : ()\n",
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
        "Newspeak3\n'T'\nclass IfArity usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t| b |\n\t\tb := [:x | x].\n\t\ttrue ifTrue: b\n\t)\n) : ()\n",
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
        "Newspeak3\n'T'\nclass Deep usingPlatform: platform = Object new (\n) (\n\tdeepStack: n = ( ^0 = n ifTrue: [0] ifFalse: [n + (deepStack: n - 1) + n] )\n\tpublic main: p args: a = (\n\t\tp transcript show: (deepStack: 1000) printString; newLine\n\t)\n) : ()\n",
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
        "Newspeak3\n'T'\nclass Unbounded usingPlatform: platform = Object new (\n) (\n\tdown: n = ( ^1 + (down: n + 1) )\n\tpublic main: p args: a = (\n\t\tp transcript show: 'start'; newLine.\n\t\tdown: 0\n\t)\n) : ()\n",
        1,
        "start\n",
        "Uncaught Newspeak error: stack overflow: recursion too deep\n",
    );
}

/// `do:` still re-enters the run loop from Rust for every element, so a
/// recursion through it is bounded by the second limit -- and ends in its
/// error, not in a native stack overflow, on the CLI's interpreter thread.
#[test]
fn a_recursion_through_do_is_bounded_by_the_native_limit() {
    assert_outcome(
        "do-recursion",
        "Newspeak3\n'T'\nclass DoDeep usingPlatform: platform = Object new (\n) (\n\td: n = ( #(1) do: [:e | d: n + 1] )\n\tpublic main: p args: a = ( d: 0 )\n) : ()\n",
        1,
        "",
        "Uncaught Newspeak error: stack overflow: too many nested native calls\n",
    );
}

/// Instantiation re-enters the loop for each initializer level, so a class
/// whose slot initializer instantiates it again is bounded by the same limit.
#[test]
fn a_recursive_instantiation_is_bounded_by_the_native_limit() {
    assert_outcome(
        "instantiation-recursion",
        "Newspeak3\n'T'\nclass Tower usingPlatform: platform = Object new (\n) (\n\tpublic class Floor = Object new (\n\t\t| above = Floor new. |\n\t) (\n\t)\n\tpublic main: p args: a = ( Floor new )\n) : ()\n",
        1,
        "",
        "Uncaught Newspeak error: stack overflow: too many nested native calls\n",
    );
}
