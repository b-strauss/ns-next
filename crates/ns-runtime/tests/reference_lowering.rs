//! The reference lowering (`docs/design/compiler.md`, "The reference
//! lowering"): programs answer alike under both lowerings, the reference
//! lowering's bodies have the shape the design promises, and every corpus
//! module the resolver accepts compiles under both.

use indoc::indoc;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ns_runtime::bytecode::Instr;
use ns_runtime::{
    HostResult, HostSignal, Interpreter, Lowering, Root, SourceOrigin, Symbol, Value,
};

fn load(src: &str, lowering: Lowering) -> (Interpreter, Root) {
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

/// `HostSignal` carries no `Debug` impl, so `Result::expect` is unusable on
/// a `HostResult` -- unwrap by hand.
fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(e)) => panic!("{what}: unexpected Newspeak error: {}", e.message),
    }
}

/// Every construct whose lowering the two differ on: inlined conditionals,
/// loops and `and:`/`or:`, a `^` out of an inlined block, slot
/// initializers, a cascade, a chained assignment.
const PROGRAMS: &str = indoc! {"
    Newspeak3
    'T'
    class Programs = Object new (
        | a = 3. b = a + 4. |
    ) (
        fact: n = ( ^n = 0 ifTrue: [1] ifFalse: [n * (self fact: n - 1)] )
        public factorial = ( ^self fact: 10 )
        public loop = ( | i sum | i := 0. sum := 0. [i < 10] whileTrue: [i := i + 1. sum := sum + i]. ^sum )
        public early = ( | c | c := OrderedCollection new. c add: 1; add: 2; add: 3. c do: [:e | e = 2 ifTrue: [^e * 100]]. ^0 )
        public logic = ( ^(((3 < 4) and: [4 < 5]) and: [(3 > 4) or: [true]]) ifTrue: [1] ifFalse: [0] )
        public slots = ( ^a + b )
        public cascade = ( | c n | c := OrderedCollection new. c add: 1; add: 2; add: 3. n := 0. c do: [:e | n := n + e]. ^n )
        public chained = ( | t u | u := t := 5. ^t + u )
    ) : ()
"};

/// Each answer is worked out by hand from the program, not recorded from a
/// run: 10!, 1 + ... + 10, the `^` out of `do:` at the element 2, a true
/// conjunction, 3 + 7, 1 + 2 + 3, 5 + 5.
#[test]
fn every_program_answers_the_same_under_both_lowerings() {
    let expected = [
        ("factorial", 3_628_800),
        ("loop", 55),
        ("early", 200),
        ("logic", 1),
        ("slots", 10),
        ("cascade", 6),
        ("chained", 10),
    ];
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let (mut interp, class) = load(PROGRAMS, lowering);
        let instance = expect_ok(interp.instantiate(&class, &[]), "instantiate");
        for (selector, answer) in expected {
            let value = expect_ok(
                interp.send(&instance, Symbol::intern(selector), &[]),
                selector,
            );
            assert!(
                interp.enter(|m| matches!(m.fetch(&value), Value::SmallInt(n) if n == answer)),
                "{selector} under {lowering:?} did not answer {answer}"
            );
        }
    }
}

/// An initializer is each own slot's initializer followed by `InitSlot`, one
/// statement apiece, then the `initExprs`, then `self` -- with nothing inlined
/// and every value in a register of its own.
#[test]
fn a_reference_initializer_initializes_each_slot_then_answers_self() {
    let src = indoc! {"
        Newspeak3
        'T'
        class X = Object new (
            | a = 1. b ::= 2. |
            a printString.
        ) (
        ) : ()
    "};
    let (mut interp, class) = load(src, Lowering::Reference);
    let code = interp.enter(|m| {
        let Value::Obj(class) = m.fetch(&class) else {
            panic!("load_module answers a class object")
        };
        let mixin = m
            .heap()
            .class_application(class)
            .expect("a class")
            .mixin
            .clone();
        mixin.initializer_code.clone().expect("an initializer")
    });
    assert!(matches!(code.instrs[0], Instr::LoadLiteral { dst: 0, .. }));
    assert_eq!(
        code.instrs[1],
        Instr::InitSlot {
            src: 0,
            own_index: 0
        }
    );
    assert!(matches!(code.instrs[2], Instr::LoadLiteral { dst: 1, .. }));
    assert_eq!(
        code.instrs[3],
        Instr::InitSlot {
            src: 1,
            own_index: 1
        }
    );
    assert!(matches!(
        &code.instrs[4],
        Instr::SendLexical { selector, arg_regs, dst: 2, .. }
            if *selector == Symbol::intern("a") && arg_regs.is_empty()
    ));
    assert!(matches!(
        &code.instrs[5],
        Instr::SendPublic { selector, receiver_reg: 2, dst: 3, .. }
            if *selector == Symbol::intern("printString")
    ));
    assert_eq!(code.instrs[6], Instr::LoadSelf { dst: 4 });
    assert_eq!(code.instrs[7], Instr::ImplicitReturn { reg: 4 });
    assert_eq!(code.instrs.len(), 8);
    assert_eq!(code.statement_pc, vec![(0, 0..2), (1, 2..4), (2, 4..6)]);
}

fn collect_ns_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir)
        .expect("read corpus directory")
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            collect_ns_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "ns") {
            out.push(path);
        }
    }
}

/// "Compilation cannot fail" (`docs/design/compiler.md`) over the whole
/// corpus rather than the six modules that run, under both lowerings -- which
/// also runs the reference lowering's shape check on every body it compiles.
/// Loading compiles every method and every per-mixin body, nothing runs.
#[test]
fn every_resolvable_corpus_module_compiles_under_both_lowerings() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/modules");
    let mut files = Vec::new();
    collect_ns_files(&dir, &mut files);
    let mut compiled = 0;
    for path in &files {
        let source = std::fs::read_to_string(path).expect("read module");
        let Ok(module) = ns_syntax::parse(&source) else {
            continue; // the blacklisted file
        };
        let Ok(resolved) = ns_runtime::resolve(
            module,
            source.as_str().into(),
            SourceOrigin::File(path.clone()),
        ) else {
            continue;
        };
        for lowering in [Lowering::Optimized, Lowering::Reference] {
            Interpreter::bootstrap_with(lowering).load_module(&resolved);
        }
        compiled += 1;
    }
    // 189 of the 211 modules resolve today (measured while planning). A
    // floor, never lowered: a module that stops resolving or stops being found
    // would otherwise drop out of this check silently.
    assert!(
        compiled >= 189,
        "expected at least 189 resolvable corpus modules, compiled {compiled} -- \
         raise this floor when more resolve, never lower it"
    );
}
