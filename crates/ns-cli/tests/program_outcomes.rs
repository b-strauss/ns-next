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

use common::ScratchDirectory;

/// Runs `source` with `ns-next run` under each lowering and asserts all three
/// streams exactly.
fn assert_outcome(name: &str, source: &str, exit: i32, stdout: &str, stderr: &str) {
    let directory = ScratchDirectory::new(name);
    let path = directory.join(&format!("{name}.ns"));
    std::fs::write(&path, source).expect("write source");
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

/// `newspeak-spec.pdf` §3.2: metaclasses are flat, so a class side is not
/// inherited down the subclass chain. `SpecialWidget` declares no class
/// side of its own; its superclass `Widget` does, but `SpecialWidget
/// describe` must not reach it -- an ordinary `doesNotUnderstand:`, exiting
/// non-zero, is the only fixture-shaped outcome this program has, which is
/// why it is pinned here rather than as a `goldens/**/*.ns` fixture:
/// `run_fixtures.rs` requires every one of those to succeed.
#[test]
fn a_class_side_is_not_inherited_by_a_subclass_with_none_of_its_own() {
    assert_outcome(
        "class_side_not_inherited",
        indoc! {"
            Newspeak3
            'Test'
            class ClassSideNotInherited usingPlatform: platform = Object new (| transcript ::= platform transcript. |) (
                public class Widget new = Object new () (
                ) : (
                    public describe = ( ^'a widget class' )
                )
                public class SpecialWidget new = Widget new () (
                )
                public main: platform args: args = (
                    transcript show: SpecialWidget describe; newLine.
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: an object does not understand #describe\n",
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

/// §6.1: "It is a run time error if the message selector specified by the
/// inheritance clause is not the selector of the primary factory of the
/// class *C*."
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

/// `newspeak-spec.pdf` §5.10: "If a public or protected method f named s is
/// defined for (3.5) S, the value of the send is the result of invoking f" --
/// a private method of the superclass is not one, so `super p` falls to "the
/// #doesNotUnderstand: method defined for S", which is `Object`'s.
#[test]
fn a_super_send_does_not_reach_a_private_method() {
    assert_outcome(
        "super-private",
        indoc! {"
            Newspeak3
            'T'
            class SuperPrivate usingPlatform: platform = Object new (
            ) (
                class SuperBase = Object new () (
                    private p = ( ^10 )
                )
                class SuperChild = SuperBase new () (
                    public run = ( ^super p )
                )
                public main: pl args: a = (
                    SuperChild new run
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: an object does not understand #p\n",
    );
}

/// `newspeak-spec.pdf` §5.7: no lexically enclosing declaration declares
/// `p` -- `Base2` is `Child2`'s superclass, not an enclosing class -- so `^p`
/// is `self p`, that is `outer Child2 p` (§5.8). §5.9: `Child2` declares no
/// private `p`, and `lookupProtected` passes `Base2`'s private `p` over, so
/// the send falls to `doesNotUnderstand:`.
#[test]
fn a_superclasss_private_method_is_not_lexically_visible() {
    assert_outcome(
        "inherited-private",
        indoc! {"
            Newspeak3
            'T'
            class InheritedPrivate usingPlatform: platform = Object new (
            ) (
                class Base2 = Object new () (
                    private p = ( ^10 )
                )
                class Child2 = Base2 new () (
                    public run = ( ^p )
                )
                public main: pl args: a = (
                    Child2 new run
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: an object does not understand #p\n",
    );
}

/// `newspeak-spec.pdf` §5.7: no lexically enclosing declaration declares `n`
/// -- `Enclosing` only inherits it from `OuterBase` -- so `^n` is "equivalent
/// to a self send", and on figure 4, "Only if this fails is the receiver's
/// inheritance chain followed": the receiver's, `Nested`'s, which has no
/// `n`. §2.3.1: "After that, no further lookup is done."
#[test]
fn an_enclosing_classs_inherited_member_is_not_lexically_visible() {
    assert_outcome(
        "enclosing-inherited",
        indoc! {"
            Newspeak3
            'T'
            class EnclosingInherited usingPlatform: platform = Object new (
            ) (
                class OuterBase = Object new () (
                    public n = ( ^1 )
                )
                class Enclosing = OuterBase new () (
                    class Nested = Object new () (
                        public implicit = ( ^n )
                    )
                    public test = ( ^Nested new implicit )
                )
                public main: pl args: a = (
                    Enclosing new test
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: an object does not understand #n\n",
    );
}

/// `newspeak-spec.pdf` §6: a nested class declaration "implicitly defines a
/// method with the same name and accessibility" in its enclosing class, so a
/// `private class Hidden` defines a private accessor, which §5.5's
/// `lookupPublic` never answers.
#[test]
fn a_private_nested_class_is_unreachable_from_outside() {
    assert_outcome(
        "private-nested",
        indoc! {"
            Newspeak3
            'T'
            class PrivateNested usingPlatform: platform = Object new (
            ) (
                class Holder = Object new () (
                    private class Hidden = Object new () ()
                )
                public main: pl args: a = (
                    Holder new Hidden
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: an object does not understand #Hidden\n",
    );
}

/// `newspeak-spec.pdf` §6.3.2: an immutable slot "may be accessed exclusively
/// by invoking a getter method" -- "There is no setter method associated with
/// the slot", however public the slot is.
#[test]
fn an_immutable_slot_has_no_setter() {
    assert_outcome(
        "immutable-setter",
        indoc! {"
            Newspeak3
            'T'
            class ImmutableSetter usingPlatform: platform = Object new (
            ) (
                class Holder = Object new (| public y = 1. |) ()
                public main: pl args: a = (
                    Holder new y: 2
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: an object does not understand #y:\n",
    );
}

/// `newspeak-spec.pdf` §6.3.1: "If no access modifier is declared, the
/// default is protected", and §6.3.2 gives the getter "the same"
/// accessibility as the slot -- so §5.5's `lookupPublic` finds a protected
/// method and answers "Undefined".
#[test]
fn a_protected_slot_does_not_answer_an_explicit_receiver() {
    assert_outcome(
        "protected-slot",
        indoc! {"
            Newspeak3
            'T'
            class ProtectedSlot usingPlatform: platform = Object new (
            ) (
                class Holder = Object new (| w = 3. |) ()
                public main: pl args: a = (
                    Holder new w
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: an object does not understand #w\n",
    );
}

/// A user subclass of a builtin inherits the builtin's natives, but its
/// instances carry no payload of the builtin's kind, so every such native
/// fails on them and the send falls to `Object`'s: `=` answers identity and
/// `printString` the generic default, through an explicit receiver, a `self`
/// send, an implicit-receiver send and a `super` send alike, while `size`,
/// which `Object` does not have, ends in `doesNotUnderstand:`. The
/// `transcript` stream shares the `Platform` class with the platform
/// singleton and has no `=` of its own, so `=` is `Object`'s identity, false
/// for two fresh streams. Step 3a keeps every one of these outcomes
/// (`docs/design/dispatch.md`, "A primitive can fail").
#[test]
fn a_native_that_fails_on_its_receiver_falls_to_objects() {
    assert_outcome(
        "native-fallback",
        indoc! {"
            Newspeak3
            'T'
            class Sub usingPlatform: platform = Object new (
            ) (
                class Text = String new () (
                    public viaSelf = ( ^self printString )
                    public viaSuper = ( ^super printString )
                    public viaLexical = ( ^printString )
                )
                public main: p args: a = (
                    | t |
                    t := Text new.
                    p transcript show: (t = t) printString; newLine.
                    p transcript show: t printString; newLine.
                    p transcript show: t viaSelf; newLine.
                    p transcript show: t viaSuper; newLine.
                    p transcript show: t viaLexical; newLine.
                    p transcript show: (p transcript = p transcript) printString; newLine.
                    p transcript show: t size printString; newLine
                )
            ) : ()
        "},
        1,
        "true\nan Object\nan Object\nan Object\nan Object\nfalse\n",
        "Uncaught Newspeak error: an object does not understand #size\n",
    );
}

/// `newspeak-spec.pdf` §5.5: `Object`'s `doesNotUnderstand:` must be
/// protected, and lookupPublic is "Undefined, if the mixin of C defines a
/// method m with selector n and protected access". So an explicit-receiver
/// send of `doesNotUnderstand:` finds nothing, and `Object`'s
/// `doesNotUnderstand:` -- found by the access-blind "defined for" walk --
/// is invoked for that send, whose selector is `doesNotUnderstand:`.
#[test]
fn an_explicit_does_not_understand_send_is_refused_as_protected() {
    assert_outcome(
        "explicit-dnu",
        indoc! {"
            Newspeak3
            'T'
            class Dnu usingPlatform: platform = Object new (
            ) (
                public main: p args: a = (
                    3 doesNotUnderstand: 5
                )
            ) : ()
        "},
        1,
        "",
        "Uncaught Newspeak error: an object does not understand #doesNotUnderstand:\n",
    );
}

/// One site that reaches a user `doesNotUnderstand:`, the setter of a
/// mutable slot named `doesNotUnderstand` (§6.3.2: a method like any other,
/// so it is the handler, and answers the receiver), and `Object`'s own
/// handler, which ends the run. A cache that recorded one handler must not
/// answer another receiver with it (`docs/design/dispatch.md`, "3b — The
/// caches").
#[test]
fn a_does_not_understand_site_with_three_kinds_of_handler() {
    assert_outcome(
        "cache_dnu_handlers",
        indoc! {"
            Newspeak3
            'Test'
            class CacheDoesNotUnderstandHandlers usingPlatform: thePlatform = Object new (| transcript = thePlatform transcript. |) (
                class Catcher = Object new () (
                    public doesNotUnderstand: message = ( ^'caught ', (message at: 1) printString )
                )
                class Recorder = Object new (| public doesNotUnderstand ::= nil. |) (
                )
                public main: platform args: args = (
                    | recorder |
                    recorder := Recorder new.
                    {Catcher new. recorder. Catcher new. recorder} do: [:each |
                        transcript show: (each zork == each) printString; newLine].
                    transcript show: (recorder doesNotUnderstand at: 1) printString; newLine.
                    transcript show: (recorder doesNotUnderstand at: 2) size printString; newLine.
                    {Catcher new. 3} do: [:each | transcript show: each zork printString; newLine].
                )
            ) : ()
        "},
        1,
        "false\ntrue\nfalse\ntrue\n#zork\n0\n'caught #zork'\n",
        "Uncaught Newspeak error: an object does not understand #zork\n",
    );
}

/// A primitive that fails where its entry was recorded: `=` and
/// `printString` on a user subclass of `String`, whose instances carry no
/// string, fall to `Object`'s; `show:` answers two transcript streams and
/// fails on the platform object, which shares their class, so the third send
/// at the same site -- the same dispatch id -- ends in `doesNotUnderstand:`
/// (`docs/design/dispatch.md`, "A primitive can fail").
#[test]
fn a_cached_primitive_that_fails_on_a_receiver_of_the_same_class() {
    assert_outcome(
        "cache_failing_primitive",
        indoc! {"
            Newspeak3
            'Test'
            class CacheFailingPrimitive usingPlatform: thePlatform = Object new (| transcript = thePlatform transcript. |) (
                class Text = String new () (
                )
                public main: platform args: args = (
                    | text |
                    text := Text new.
                    {'abc'. text. 'abc'. text} do: [:each |
                        transcript show: (each = each) printString; show: ' '; show: each printString; newLine].
                    {platform transcript. platform. platform transcript} do: [:each |
                        transcript show: (each = each) printString; newLine].
                    {'abc'. #(1 2)} do: [:each | transcript show: each size printString; newLine].
                    {platform transcript. platform transcript. platform} do: [:each | each show: 'shown'. each newLine].
                )
            ) : ()
        "},
        1,
        "true 'abc'\ntrue an Object\ntrue 'abc'\ntrue an Object\ntrue\ntrue\ntrue\n3\n2\nshown\nshown\n",
        "Uncaught Newspeak error: an object does not understand #show:\n",
    );
}
