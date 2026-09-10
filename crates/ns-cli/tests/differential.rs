//! Every fixture, plus inline sources that must not be fixtures (they exit
//! non-zero), run under both engines; exit status, stdout, AND stderr must
//! match byte for byte. The Milestone 1 tree-walker is the oracle (M2 spec,
//! "Testing").

use std::path::{Path, PathBuf};
use std::process::Command;

/// Same normalization as `run_fixtures.rs`: `.expected` files may be
/// checked out with `\r\n` on Windows.
fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

pub struct Outcome {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_with_engine(path: &Path, engine: &str) -> Outcome {
    let output = Command::new(env!("CARGO_BIN_EXE_ns-next"))
        .arg("run")
        .arg(path)
        .arg("--engine")
        .arg(engine)
        .output()
        .expect("run ns-next");
    Outcome {
        ok: output.status.success(),
        stdout: normalize_newlines(&String::from_utf8_lossy(&output.stdout)),
        stderr: normalize_newlines(&String::from_utf8_lossy(&output.stderr)),
    }
}

/// Runs `path` under both engines, asserts they agree on everything
/// observable, and returns the bytecode engine's outcome for further
/// assertions.
pub fn assert_engines_agree(path: &Path) -> Outcome {
    let bc = run_with_engine(path, "bytecode");
    let tw = run_with_engine(path, "tree-walk");
    assert_eq!(
        bc.ok,
        tw.ok,
        "{}: exit status differs (bytecode stderr: {}; tree-walk stderr: {})",
        path.display(),
        bc.stderr,
        tw.stderr
    );
    assert_eq!(
        bc.stdout,
        tw.stdout,
        "{}: stdout differs between engines",
        path.display()
    );
    assert_eq!(
        bc.stderr,
        tw.stderr,
        "{}: stderr differs between engines",
        path.display()
    );
    bc
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-fixtures")
}

/// Writes `src` to a per-test temp file (unique per test name and process,
/// so parallel tests never collide) and returns its path.
fn write_temp_source(name: &str, src: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ns-next-differential-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join(format!("{name}.ns"));
    std::fs::write(&path, src).expect("write temp source");
    path
}

pub fn assert_engines_agree_on_source(name: &str, src: &str) -> Outcome {
    let path = write_temp_source(name, src);
    let outcome = assert_engines_agree(&path);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
    outcome
}

#[test]
fn every_tests_fixtures_ns_file_behaves_identically_under_both_engines() {
    let mut checked = 0;
    for entry in std::fs::read_dir(fixtures_dir()).expect("read tests-fixtures") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("ns") {
            continue;
        }
        let outcome = assert_engines_agree(&path);
        assert!(
            outcome.ok,
            "{} failed under both engines: {}",
            path.display(),
            outcome.stderr
        );
        checked += 1;
    }
    // The floor is the fixture count as it stands, not a comfortable lower
    // bound: this test's whole job is to notice that a fixture stopped
    // being run under both engines, and a slack floor lets one disappear in
    // silence. Raise it in the same commit that adds a fixture.
    assert!(
        checked >= 19,
        "expected every one of the 19 tests-fixtures/*.ns fixtures to run \
         under both engines, checked {checked} -- raise this floor when \
         adding a fixture, never lower it to match what was found"
    );
}

#[test]
fn the_instance_init_fixture_runs_its_initializer_once_under_bytecode() {
    let outcome = run_with_engine(&fixtures_dir().join("instance_init.ns"), "bytecode");
    assert!(outcome.ok, "{}", outcome.stderr);
    assert_eq!(outcome.stdout, "init ran\nhi 2\n");
}

#[test]
fn a_does_not_understand_error_is_identical_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "dnu",
        "Newspeak3\n'T'\nclass Dnu usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t3 fooBar\n\t)\n) : ()\n",
    );
    assert!(!outcome.ok);
    assert!(
        outcome.stderr.contains("does not understand #fooBar"),
        "{}",
        outcome.stderr
    );
}

/// A block created while its class initializes, whose `^` runs after the
/// initializer has returned. Its home is dead, so this is §7.2's
/// `cannotReturn` -- under both engines, from the shared `raise_return`.
/// Before the activation stack an initializer frame was never marked
/// uncontinuable and the return escaped the entry send instead.
#[test]
fn a_return_from_a_block_whose_initializer_has_finished_is_cannot_return() {
    let outcome = assert_engines_agree_on_source(
        "init_block_return",
        "Newspeak3\n'Test'\nclass InitBlockReturn usingPlatform: platform = Object new (\n\
         \t| blk = [ ^2 ]. |\n) (\n\
         \tpublic main: p args: a = (\n\
         \t\tblk value.\n\
         \t\tp transcript show: 'unreachable'; newLine\n\
         \t)\n) : ()\n",
    );
    assert!(!outcome.ok, "a dead home must be an error");
    assert!(
        outcome.stderr.contains("cannotReturn"),
        "expected cannotReturn, got: {}",
        outcome.stderr
    );
    assert!(!outcome.stdout.contains("unreachable"));
}

#[test]
fn a_non_boolean_receiver_of_an_inlined_selector_does_not_understand_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "if-dnu",
        "Newspeak3\n'T'\nclass IfDnu usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t3 ifTrue: ['a'] ifFalse: ['b']\n\t)\n) : ()\n",
    );
    assert!(!outcome.ok);
    assert!(
        outcome
            .stderr
            .contains("does not understand #ifTrue:ifFalse:"),
        "{}",
        outcome.stderr
    );
}

#[test]
fn a_non_boolean_while_condition_fails_with_must_be_boolean_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "while-must-be-boolean",
        "Newspeak3\n'T'\nclass WhileMbb usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\tp transcript show: 'before'; newLine.\n\t\t[p transcript show: 'cond'; newLine. 3] whileTrue: [nil]\n\t)\n) : ()\n",
    );
    assert!(!outcome.ok);
    assert_eq!(
        outcome.stdout, "before\ncond\n",
        "the condition body must run exactly once"
    );
    assert!(
        outcome
            .stderr
            .contains("does not understand #mustBeBoolean"),
        "{}",
        outcome.stderr
    );
}

#[test]
fn a_variable_block_receiver_takes_the_native_loop_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "while-variable",
        "Newspeak3\n'T'\nclass WhileVar usingPlatform: platform = Object new (\n\t| n ::= 0. |\n) (\n\tpublic main: p args: a = (\n\t\t| cond |\n\t\tcond := [n < 3].\n\t\tcond whileTrue: [n := n + 1].\n\t\tp transcript show: n printString; newLine\n\t)\n) : ()\n",
    );
    assert!(outcome.ok, "{}", outcome.stderr);
    assert_eq!(outcome.stdout, "3\n");
}

/// `and:`/`or:` short-circuiting (which side
/// effects run) and `truthy_arg`'s coercion of a non-Boolean block value are
/// the inlined shapes with no end-to-end coverage above.
#[test]
fn and_or_short_circuit_and_coerce_identically_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "and-or",
        "Newspeak3\n'T'\nclass AndOr usingPlatform: platform = Object new (\n\t| log ::= ''. |\n) (\n\tnote: s = ( log := log, s. ^true )\n\tpublic main: p args: a = (\n\t\tp transcript show: (true and: [self note: 'A']) printString; newLine.\n\t\tp transcript show: (false and: [self note: 'B']) printString; newLine.\n\t\tp transcript show: (true or: [self note: 'C']) printString; newLine.\n\t\tp transcript show: (false or: [self note: 'D']) printString; newLine.\n\t\tp transcript show: (true and: [3]) printString; newLine.\n\t\tp transcript show: log; newLine\n\t)\n) : ()\n",
    );
    assert!(outcome.ok, "{}", outcome.stderr);
    assert_eq!(
        outcome.stdout, "true\nfalse\ntrue\ntrue\nfalse\nAD\n",
        "only the non-short-circuited arms may run, and a non-Boolean block value coerces to false"
    );
}

/// A `^` inside an inlined loop body unwinds out of the whole loop, and a
/// closure built inside an inlined branch still reaches the enclosing frame.
#[test]
fn a_return_out_of_an_inlined_loop_body_agrees_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "inlined-return",
        "Newspeak3\n'T'\nclass InlinedReturn usingPlatform: platform = Object new (\n) (\n\tcount: limit = (\n\t\t| i |\n\t\ti := 0.\n\t\t[true] whileTrue: [\n\t\t\ti := i + 1.\n\t\t\ti > limit ifTrue: [^i]].\n\t\t^0\n\t)\n\tcapture: x = (\n\t\t| c |\n\t\tx > 0 ifTrue: [c := [x * 2]] ifFalse: [c := [x]].\n\t\t^c value\n\t)\n\tpublic main: p args: a = (\n\t\tp transcript show: (self count: 3) printString; newLine.\n\t\tp transcript show: (self capture: 5) printString; newLine\n\t)\n) : ()\n",
    );
    assert!(outcome.ok, "{}", outcome.stderr);
    assert_eq!(outcome.stdout, "4\n10\n");
}

/// The shape class that produced the C1 defect: a send's operands mutating
/// each other's inputs. Every consumer reads its operand registers when the
/// *instruction* executes, so a named local read earlier in the same
/// statement must not still be sitting in that local's own storage. One
/// case per consumer -- keyword arguments, a binary send's right operand
/// mutating the left, dynamic-array elements, and an inlined arm (whose
/// statements run inside the enclosing statement's operand sequence).
#[test]
fn operands_that_mutate_each_other_evaluate_in_the_same_order_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "operand-order",
        "Newspeak3\n'T'\nclass OperandOrder usingPlatform: platform = Object new (\n) (\n\tfirst: x second: y = ( ^x )\n\tkeywordArgs = ( | t | t := 1. ^self first: t second: (t := 9) )\n\tbinaryOperand = ( | t | t := 2. ^t + (t := 30) )\n\tarrayElements = ( | t | t := 1. ^{t. t := 5. t} at: 1 )\n\tinlinedArm = ( | t | t := 2. ^t + (true ifTrue: [t := 9. 1] ifFalse: [2]) )\n\tpublic main: p args: a = (\n\t\tp transcript show: self keywordArgs printString; newLine.\n\t\tp transcript show: self binaryOperand printString; newLine.\n\t\tp transcript show: self arrayElements printString; newLine.\n\t\tp transcript show: self inlinedArm printString; newLine\n\t)\n) : ()\n",
    );
    assert!(outcome.ok, "{}", outcome.stderr);
    assert_eq!(
        outcome.stdout, "1\n32\n1\n3\n",
        "every operand keeps the value it had when it was evaluated"
    );
}

/// The same invariant when the write happens inside a *callee*: a closure
/// built in an earlier statement assigns the local while a send in this
/// statement is running, so no assignment appears in the operand list at
/// all. Covers a cascade receiver (live across the cascade's own sends) and
/// an assignment's value handed on through a setter send.
#[test]
fn an_operand_written_by_a_callee_agrees_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "operand-callee",
        "Newspeak3\n'T'\nclass CalleeClobber usingPlatform: platform = Object new (\n\t| c ::= nil. |\n) (\n\tfirst: x second: y = ( ^x )\n\thook: v = ( c value. ^v )\n\tsetterValue = ( | t | t := 1. c := [t := 9]. ^self first: (hook := t) second: 1 )\n\targThroughASend = ( | t | t := 1. c := [t := 9]. ^self first: t second: (self hook: 0) )\n\tcascadeReceiver = (\n\t\t| t orig |\n\t\torig := OrderedCollection new.\n\t\tt := orig.\n\t\tc := [t := OrderedCollection new].\n\t\tt add: 1; add: (self hook: 2).\n\t\t^orig size\n\t)\n\tpublic main: p args: a = (\n\t\tp transcript show: self setterValue printString; newLine.\n\t\tp transcript show: self argThroughASend printString; newLine.\n\t\tp transcript show: self cascadeReceiver printString; newLine\n\t)\n) : ()\n",
    );
    assert!(outcome.ok, "{}", outcome.stderr);
    assert_eq!(
        outcome.stdout, "1\n1\n2\n",
        "a callee's write to a local must not reach an operand already evaluated"
    );
}

/// A bare `super` parses and resolves (Milestone 1 does not lexer-reserve
/// it) but has no binding, so it is a program-level error raised when it is
/// evaluated -- never a compile-time panic under the default engine.
#[test]
fn a_bare_super_is_the_same_program_error_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "bare-super",
        "Newspeak3\n'T'\nclass BareSuper usingPlatform: platform = Object new (\n) (\n\tpublic main: p args: a = (\n\t\t^super\n\t)\n) : ()\n",
    );
    assert!(!outcome.ok);
    assert_eq!(
        outcome.stderr, "Uncaught Newspeak error: internal: unresolved identifier `super`\n",
        "the deferred literal must raise eval.rs's own error, word for word"
    );
}

/// The error is raised where the identifier is *evaluated*, not when the
/// method is compiled: everything before it still runs.
#[test]
fn a_bare_super_raises_only_when_it_is_reached_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "bare-super-deferred",
        "Newspeak3\n'T'\nclass DeferredSuper usingPlatform: platform = Object new (\n) (\n\tunreached = ( ^super )\n\tpublic main: p args: a = (\n\t\tp transcript show: 'before'; newLine.\n\t\t^self unreached\n\t)\n) : ()\n",
    );
    assert!(!outcome.ok);
    assert_eq!(outcome.stdout, "before\n");
}

/// `newspeak-spec.pdf` §6.1: "It is a run time error if the message
/// determined by the inheritance clause is not the primary factory message
/// of C." `Derived`'s clause sends `new`, but `Base`'s primary factory is
/// `withBase:` -- a program error raised from the shared orchestration, so
/// both engines print the identical stderr and neither panics.
#[test]
fn a_superclass_clause_naming_the_wrong_factory_fails_identically_under_both_engines() {
    let outcome = assert_engines_agree_on_source(
        "wrong-superclass-factory",
        "Newspeak3\n'T'\nclass WrongFactory usingPlatform: platform = Object new (\n) (\n\tpublic class Base withBase: b = Object new (| v ::= b. |) (\n\t)\n\tpublic class Derived = Base new (\n\t) (\n\t)\n\tpublic main: p args: a = (\n\t\t^Derived new\n\t)\n) : ()\n",
    );
    assert!(!outcome.ok);
    assert!(
        outcome.stderr.contains(
            "the superclass clause of class Derived sends #new, \
             but the primary factory of class Base is #withBase:"
        ),
        "{}",
        outcome.stderr
    );
}
