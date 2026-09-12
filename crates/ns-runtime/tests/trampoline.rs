//! The trampoline (`docs/design/execution.md`, "The trampoline (1b)"): sends
//! run on the activation stack, not the Rust stack, so recursion depth is no
//! longer bounded by the native stack -- and, once the bounds arrive, is an
//! ordinary Newspeak error under both lowerings.

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
/// a test thread holds when every send recursed on it, and nothing but
/// activation-stack entries once none does. Under the optimized lowering no
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
/// each held in a variable -- runs through the natives, and after this task
/// none of them calls back into the loop, so fifty thousand levels fit on a
/// test thread's stack under either lowering.
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
