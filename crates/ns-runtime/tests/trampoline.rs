//! The trampoline (`docs/design/execution.md`, "The trampoline (1b)"): sends
//! run on the activation stack, not the Rust stack, so recursion depth is
//! bounded by a count rather than by the native stack, and running past it is
//! an ordinary Newspeak error under both lowerings.

use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::{ClassRef, Interpreter, Lowering, Signal, SourceOrigin, Symbol, Value};

fn load(src: &str, lowering: Lowering) -> (Interpreter, ClassRef) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(src),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap_with(lowering);
    let class = interp.load_module(&resolved);
    (interp, class)
}

/// `Signal` carries no `Debug` impl, so `Result::expect` is unusable on an
/// `EvalResult` -- unwrap by hand.
fn expect_ok(result: Result<Value, Signal>, what: &str) -> Value {
    match result {
        Ok(v) => v,
        Err(Signal::Return(..)) => panic!("{what}: unexpected Signal::Return"),
        Err(Signal::Error(e)) => panic!("{what}: unexpected Signal::Error: {}", e.message),
    }
}

fn small_int(v: Value) -> i64 {
    match v {
        Value::SmallInt(n) => n,
        _ => panic!("expected a SmallInteger"),
    }
}

const DEPTH: &str = "Newspeak3\n'T'\nclass Depth = Object new (\n) (\n\
    \tpublic down: n = ( n = 0 ifTrue: [^0]. ^down: n - 1 )\n\
    \tpublic logic: n = ( ^n = 0 or: [(logic: n - 1) and: [true]] )\n\
    \tpublic loop: n = ( [n > 0] whileTrue: [^loop: n - 1]. ^0 )\n\
    ) : ()";

/// Fifty thousand nested method activations: far past what the Rust stack of
/// a test thread would hold if every send recursed on it, and nothing but
/// activation-stack entries since none does. Under the optimized lowering no
/// native re-enters Newspeak here, since every control-flow send is inlined.
#[test]
fn a_deep_method_recursion_runs_on_the_activation_stack() {
    let (mut interp, class) = load(DEPTH, Lowering::Optimized);
    let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
    for selector in ["down:", "logic:", "loop:"] {
        let result = expect_ok(
            interp.send(
                instance,
                Symbol::intern(selector),
                vec![Value::SmallInt(50_000)],
            ),
            selector,
        );
        assert!(
            matches!(result, Value::SmallInt(0) | Value::Bool(true)),
            "{selector}"
        );
        assert_eq!(
            interp.heap_ref().activation_depth(),
            0,
            "{selector}: nothing left behind"
        );
    }
}

/// A `^` out of a method several activations up, through a block, lands on the
/// method it names and leaves nothing behind it on the stack.
#[test]
fn a_non_local_return_unwinds_to_its_home_on_the_activation_stack() {
    let src = "Newspeak3\n'T'\nclass R = Object new (\n) (\n\
        \tpublic find = ( self each: [:e | e = 3 ifTrue: [^e * 10]]. ^0 )\n\
        \teach: blk = ( self to: 5 do: blk )\n\
        \tto: hi do: blk = ( | i | i := 1. [i <= hi] whileTrue: [blk value: i. i := i + 1] )\n\
        ) : ()";
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load(src, lowering);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        let result = expect_ok(
            interp.send(instance, Symbol::intern("find"), vec![]),
            "find",
        );
        assert_eq!(small_int(result), 30, "{lowering:?}");
        assert_eq!(interp.heap_ref().activation_depth(), 0, "{lowering:?}");
    }
}

/// A `^` into an initializer that is still running escapes -- the known gap
/// in `docs/design/execution.md`, "Non-local return" -- and it escapes as the
/// signal it always was, not as a panic. The run the initializer belongs to
/// pops it, so by the time the signal reaches the run of the method that
/// sent `new`, its target is an activation that has returned: that run must
/// let the signal pass without reading the target. Closing the gap changes
/// what this expects; nothing else should.
#[test]
fn a_return_into_a_running_initializer_escapes_the_runs_below_it() {
    let src = "Newspeak3\n'T'\nclass E = Object new (\n) (\n\
        \tclass Inner = Object new ( | a = [^1] value. | ) ( ) : ()\n\
        \tpublic make = ( Inner new. ^0 )\n\
        ) : ()";
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load(src, lowering);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        match interp.send(instance, Symbol::intern("make"), vec![]) {
            Err(Signal::Return(..)) => {}
            Ok(_) => panic!("{lowering:?}: the `^` must escape, not be answered"),
            Err(Signal::Error(e)) => panic!("{lowering:?}: unexpected error: {}", e.message),
        }
        assert_eq!(interp.heap_ref().activation_depth(), 0, "{lowering:?}");
    }
}

/// A recursion through blocks the optimized lowering does *not* inline --
/// each held in a variable -- runs through the natives, and none of them
/// calls back into the loop, so forty thousand levels fit on a test thread's
/// stack under either lowering.
const THROUGH_NATIVES: &str = "Newspeak3\n'T'\nclass N = Object new (\n) (\n\
    \tpublic viaValue: n = ( | b | b := [:k | k = 0 ifTrue: [0] ifFalse: [b value: k - 1]]. ^b value: n )\n\
    \tpublic viaIf: n = ( | t f | t := [0]. f := [viaIf: n - 1]. ^n = 0 ifTrue: t ifFalse: f )\n\
    \tpublic viaOr: n = ( | rest | rest := [(viaOr: n - 1) = 0]. ^(n = 0 or: rest) ifTrue: [0] ifFalse: [1] )\n\
    \tpublic viaWhile: n = ( | c b | c := [n > 0]. b := [^viaWhile: n - 1]. c whileTrue: b. ^0 )\n\
    ) : ()";

#[test]
fn a_deep_recursion_through_native_block_activations_runs_on_the_activation_stack() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load(THROUGH_NATIVES, lowering);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        for selector in ["viaValue:", "viaIf:", "viaOr:", "viaWhile:"] {
            let result = expect_ok(
                interp.send(
                    instance,
                    Symbol::intern(selector),
                    vec![Value::SmallInt(40_000)],
                ),
                selector,
            );
            assert_eq!(small_int(result), 0, "{selector} under {lowering:?}");
            assert_eq!(
                interp.heap_ref().activation_depth(),
                0,
                "{selector} under {lowering:?}"
            );
        }
    }
}

const TOO_DEEP: &str = "stack overflow: recursion too deep";

fn expect_too_deep(result: Result<Value, Signal>, what: &str) {
    match result {
        Err(Signal::Error(e)) => assert_eq!(e.message, TOO_DEEP, "{what}"),
        Err(Signal::Return(..)) => panic!("{what}: a non-local return escaped"),
        Ok(_) => panic!("{what}: ran past the depth limit"),
    }
}

/// The limit is exactly `MAX_ACTIVATION_DEPTH` counted activations, and it
/// counts alike under both lowerings (`docs/design/compiler.md`, "How the
/// limit counts"). `down: k` sent from Rust makes k + 1 method activations,
/// so `down: 99_999` is the deepest that fits and `down: 100_000` one too
/// many. `logic:` and `loop:` recurse from inside an `or:` block and a
/// `whileTrue:` body, which the optimized lowering inlines and the reference
/// lowering activates -- exempt, so the boundary is the same.
#[test]
fn the_depth_limit_is_exact_and_counts_alike_under_both_lowerings() {
    assert_eq!(ns_runtime::interpreter::MAX_ACTIVATION_DEPTH, 100_000);
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load(DEPTH, lowering);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        for selector in ["down:", "logic:", "loop:"] {
            let what = format!("{selector} under {lowering:?}");
            expect_ok(
                interp.send(
                    instance,
                    Symbol::intern(selector),
                    vec![Value::SmallInt(99_999)],
                ),
                &what,
            );
            expect_too_deep(
                interp.send(
                    instance,
                    Symbol::intern(selector),
                    vec![Value::SmallInt(100_000)],
                ),
                &what,
            );
            assert_eq!(
                interp.heap_ref().activation_depth(),
                0,
                "{what}: nothing left behind"
            );
            // An overflow is an ordinary error: the interpreter goes on.
            expect_ok(
                interp.send(
                    instance,
                    Symbol::intern(selector),
                    vec![Value::SmallInt(10)],
                ),
                &what,
            );
        }
    }
}

/// A block activated through `value` is counted like any other activation,
/// so a recursion through blocks alone ends in the same error.
#[test]
fn a_recursion_through_block_values_is_bounded_too() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load(THROUGH_NATIVES, lowering);
        let instance = expect_ok(interp.instantiate(class, vec![]), "instantiate");
        expect_too_deep(
            interp.send(
                instance,
                Symbol::intern("viaValue:"),
                vec![Value::SmallInt(250_000)],
            ),
            &format!("viaValue: under {lowering:?}"),
        );
    }
}

/// A crafted artifact can send `ifTrue:` as a `SendInlinable` with a block
/// the sending activation did not create -- here the block itself, so it
/// recurses. The exemption requires the block's captured frame to be the
/// sender's (`docs/design/compiler.md`, "How the limit counts"), so this is
/// counted and ends in the depth error; without that condition it would run
/// uncounted until memory ran out.
#[test]
fn a_crafted_send_inlinable_cannot_recurse_uncounted() {
    use ns_runtime::image;
    use ns_runtime::image::opcodes::{OP_SEND_INLINABLE, OP_SEND_PUBLIC};
    let src = "Newspeak3\n'T'\nclass Crafted = Object new (\n) (\n\tpublic main = ( | b | b := [true ifTrue: b]. ^b value )\n) : ()";
    // `true ifTrue: b` is not a literal-block send, so even the reference
    // lowering compiles it to a `SendPublic`, which the mutation below turns
    // into a `SendInlinable` -- the two share one operand layout.
    let (interp, class) = load(src, Lowering::Reference);
    let mut bytes = image::build(
        interp.heap_ref(),
        None,
        &SourceOrigin::File(PathBuf::from("t.ns")),
        class,
        Symbol::intern("main"),
    )
    .expect("build");
    let report = image::inspect::inspect(&bytes).expect("inspect");
    let id = report
        .symbols
        .iter()
        .position(|s| s == "ifTrue:")
        .expect("ifTrue: is interned") as u32;
    let mut pattern = vec![OP_SEND_PUBLIC];
    pattern.extend_from_slice(&id.to_le_bytes());
    let at: Vec<usize> = bytes
        .windows(pattern.len())
        .enumerate()
        .filter(|(_, w)| *w == pattern.as_slice())
        .map(|(i, _)| i)
        .collect();
    assert_eq!(at.len(), 1, "exactly one `ifTrue:` send to mutate");
    bytes[at[0]] = OP_SEND_INLINABLE;

    let mut loaded = image::load(&bytes).expect("the mutated artifact decodes");
    let instance = expect_ok(
        loaded.interp.instantiate(loaded.root_class, vec![]),
        "instantiate",
    );
    expect_too_deep(
        loaded.interp.send(instance, Symbol::intern("main"), vec![]),
        "the crafted recursion",
    );
}
