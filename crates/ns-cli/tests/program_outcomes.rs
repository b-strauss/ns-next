//! The observable outcome -- exit status, stdout and stderr, byte for byte --
//! of programs that cannot be fixtures, because they exit non-zero or pin an
//! exact message. Every expectation here is a verdict the tree-walker and the
//! bytecode VM agreed on before the tree-walker was retired
//! (`docs/design/execution.md`), recorded so that a later change cannot move
//! it quietly. Until the reference lowering lands there is no second way to
//! run a program and compare: a change that alters one of these is a
//! behaviour change and has to be justified as one.

mod common;

use common::ScratchDir;

/// Runs `src` with `ns-next run` and asserts all three streams exactly.
fn assert_outcome(name: &str, src: &str, exit: i32, stdout: &str, stderr: &str) {
    let dir = ScratchDir::new(name);
    let path = dir.join(&format!("{name}.ns"));
    std::fs::write(&path, src).expect("write source");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(&path)
        .output()
        .expect("run ns-next");
    let actual_stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let actual_stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{name}: exit status (stderr: {actual_stderr})"
    );
    assert_eq!(actual_stdout, stdout, "{name}: stdout");
    assert_eq!(actual_stderr, stderr, "{name}: stderr");
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
