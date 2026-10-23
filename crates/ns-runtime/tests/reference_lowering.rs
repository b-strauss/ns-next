//! The reference lowering (`docs/design/compiler.md`, "The reference
//! lowering"): programs answer alike under both lowerings, the reference
//! lowering's bodies have the shape the design promises, and every corpus
//! module the resolver accepts compiles under both.

use indoc::indoc;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ns_runtime::bytecode::{CallSite, Instruction};
use ns_runtime::{HostResult, HostSignal, Interpreter, Lowering, Root, SourceOrigin, Symbol};

fn load(source: &str, lowering: Lowering) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(source),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
}

/// `HostSignal` carries no `Debug` impl, so `Result::expect` is unusable on
/// a `HostResult` -- unwrap by hand.
fn expect_ok(result: HostResult, what: &str) -> Root {
    match result {
        Ok(root) => root,
        Err(HostSignal::Return) => panic!("{what}: a non-local return escaped"),
        Err(HostSignal::Error(error)) => {
            panic!("{what}: unexpected Newspeak error: {}", error.message)
        }
    }
}

/// Every construct whose lowering the two differ on: inlined conditionals,
/// loops and `and:`/`or:`, a `^` out of an inlined block, slot
/// initializers, a cascade, a chained assignment.
const PROGRAMS: &str = indoc! {"
    class Programs = Object new (
        | a = 3. b = a + 4. |
    ) (
        fact: n = ( ^n = 0 ifTrue: [1] ifFalse: [n * (self fact: n - 1)] )
        public factorial = ( ^self fact: 10 )
        public loop = ( | i sum | i:: 0. sum:: 0. [i < 10] whileTrue: [i:: i + 1. sum:: sum + i]. ^sum )
        public early = ( | c | c:: {1. 2. 3}. c do: [:e | e = 2 ifTrue: [^e * 100]]. ^0 )
        public logic = ( ^(((3 < 4) and: [4 < 5]) and: [(3 > 4) or: [true]]) ifTrue: [1] ifFalse: [0] )
        public slots = ( ^a + b )
        public cascade = ( | c n | c:: Array new: 3. c at: 1 put: 1; at: 2 put: 2; at: 3 put: 3. n:: 0. c do: [:e | n:: n + e]. ^n )
        public chained = ( | t u | u:: t:: 5. ^t + u )
    ) : ()
"};

ns_runtime::for_each_lowering! {
    /// Each answer is worked out by hand from the program, not recorded from a
    /// run: 10!, 1 + ... + 10, the `^` out of `do:` at the element 2, a true
    /// conjunction, 3 + 7, 1 + 2 + 3, 5 + 5.
    fn every_program_answers_the_same_under_both_lowerings(lowering) {
        let expected = [
            ("factorial", 3_628_800),
            ("loop", 55),
            ("early", 200),
            ("logic", 1),
            ("slots", 10),
            ("cascade", 6),
            ("chained", 10),
        ];
        let (mut interpreter, class) = load(PROGRAMS, lowering);
        let instance = expect_ok(interpreter.instantiate(&class, &[]), "instantiate");
        for (selector, answer) in expected {
            let value = expect_ok(
                interpreter.send(&instance, Symbol::intern(selector), &[]),
                selector,
            );
            assert!(
                interpreter.enter(|mutator| mutator.fetch(&value).as_small_integer() == Some(answer)),
                "{selector} under {lowering:?} did not answer {answer}"
            );
        }
    }
}

/// An initializer is each own slot's initializer followed by `InitializeSlot`,
/// one statement apiece, then the `initExprs`, then `self` -- with nothing
/// inlined and every value in a register of its own.
#[test]
fn a_reference_initializer_initializes_each_slot_then_answers_self() {
    let source = indoc! {"
        class X = Object new (
            | a = 1. b ::= 2. |
            a printString.
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source, Lowering::Reference);
    let code = interpreter.enter(|mutator| {
        let Some(class) = mutator.fetch(&class).as_object() else {
            panic!("load_module answers a class object")
        };
        let mixin = mutator
            .heap()
            .class_application(class)
            .expect("a class")
            .mixin
            .clone();
        mixin.own_initializer_code.clone().expect("an initializer")
    });
    assert!(matches!(
        code.instruction(0),
        Instruction::LoadLiteral { destination: 0, .. }
    ));
    assert_eq!(
        code.instruction(1),
        Instruction::InitializeSlot {
            source: 0,
            own_index: 0
        }
    );
    assert!(matches!(
        code.instruction(2),
        Instruction::LoadLiteral { destination: 1, .. }
    ));
    assert_eq!(
        code.instruction(3),
        Instruction::InitializeSlot {
            source: 1,
            own_index: 1
        }
    );
    assert!(matches!(
        &code.instruction(4),
        Instruction::SendLexical { selector, argument_registers, destination: 2, .. }
            if *selector == Symbol::intern("a") && argument_registers.is_empty()
    ));
    assert!(matches!(
        &code.instruction(5),
        Instruction::SendPublic { selector, receiver_register: 2, destination: 3, .. }
            if *selector == Symbol::intern("printString")
    ));
    assert_eq!(
        code.instruction(6),
        Instruction::LoadSelf { destination: 4 }
    );
    assert_eq!(
        code.instruction(7),
        Instruction::ImplicitReturn { register: 4 }
    );
    assert_eq!(code.instructions().len(), 8);
    assert_eq!(
        code.statement_program_counter,
        vec![(0, 0..2), (1, 2..4), (2, 4..6)]
    );
}

/// The full initializer of the nested class `name` of the module whose
/// root `class` is.
fn full_initializer(
    interpreter: &mut Interpreter,
    class: &Root,
    name: &str,
) -> Rc<ns_runtime::bytecode::CompiledCode> {
    interpreter.enter(|mutator| {
        let Some(class) = mutator.fetch(class).as_object() else {
            panic!("load_module answers a class object")
        };
        let root = mutator
            .heap()
            .class_application(class)
            .expect("a class")
            .mixin
            .clone();
        mutator
            .heap()
            .nested_mixins(&root)
            .into_iter()
            .find(|nested| nested.name.as_str() == name)
            .unwrap_or_else(|| panic!("the module declares {name}"))
            .initializer_code
            .clone()
            .expect("a full initializer")
    })
}

/// The full initializer's prologue under the reference lowering: the
/// optimized lowering's shapes (`compiler.rs`), each clause argument in a
/// register nothing else writes and the parameter it reads copied first.
#[test]
fn the_reference_lowering_emits_the_same_prologue() {
    let superclass_with_argument = indoc! {"
        class Outer = Object new () (
          class A y: value = Object new () ()
          class B x: x = A y: x + 1 (| s = x. |) ()
        ) : ()
    "};
    let (mut interpreter, class) = load(superclass_with_argument, Lowering::Reference);
    let full = full_initializer(&mut interpreter, &class, "B");
    assert_eq!(
        full.instructions(),
        vec![
            Instruction::Move {
                destination: 1,
                source: 0
            },
            Instruction::LoadLiteral {
                destination: 2,
                literal: ns_runtime::bytecode::Literal::SmallInteger(1)
            },
            Instruction::SendPublic {
                receiver_register: 1,
                selector: Symbol::intern("+"),
                argument_registers: vec![2],
                destination: 3,
                call_site: CallSite::UNCACHED
            },
            Instruction::InitializeSuperclass {
                steps: 1,
                selector: Symbol::intern("y:"),
                argument_registers: vec![3],
                destination: 4
            },
            Instruction::Move {
                destination: 5,
                source: 0
            },
            Instruction::InitializeSlot {
                source: 5,
                own_index: 0
            },
            Instruction::LoadSelf { destination: 6 },
            Instruction::ImplicitReturn { register: 6 },
        ]
    );

    let two_applied_clauses = indoc! {"
        class Outer = Object new () (
          class S = Object new () ()
          class M1 = Object new () ()
          class M2 tag: value = Object new () ()
          class C = S new <: M1 new <: M2 tag: 1 () ()
        ) : ()
    "};
    let (mut interpreter, class) = load(two_applied_clauses, Lowering::Reference);
    let full = full_initializer(&mut interpreter, &class, "C");
    assert_eq!(
        full.instructions(),
        vec![
            Instruction::CheckAppliedClause {
                steps: 1,
                selector: Symbol::intern("tag:")
            },
            Instruction::CheckAppliedClause {
                steps: 2,
                selector: Symbol::NEW
            },
            Instruction::InitializeSuperclass {
                steps: 3,
                selector: Symbol::NEW,
                argument_registers: vec![],
                destination: 0
            },
            Instruction::InitializeAppliedLevel {
                steps: 2,
                argument_registers: vec![],
                destination: 1
            },
            Instruction::LoadLiteral {
                destination: 2,
                literal: ns_runtime::bytecode::Literal::SmallInteger(1)
            },
            Instruction::InitializeAppliedLevel {
                steps: 1,
                argument_registers: vec![2],
                destination: 3
            },
            Instruction::LoadSelf { destination: 4 },
            Instruction::ImplicitReturn { register: 4 },
        ]
    );
}

/// The reference lowering is the uncached oracle: every send it emits is
/// `CallSite::UNCACHED`, and none of its bodies has a family.
#[test]
fn the_reference_lowering_emits_every_send_uncached() {
    let source = indoc! {"
        class Subject = Object new (
        ) (
          public run: items = (
            items do: [:each | each printString].
            ^items size
          )
        ) : ()
    "};
    let (mut interpreter, class) = load(source, Lowering::Reference);
    let code = interpreter.enter(|mutator| {
        let Some(object) = mutator.fetch(&class).as_object() else {
            panic!("the module's class is a class object")
        };
        let application = mutator.heap().class_application(object).expect("a class");
        application
            .mixin
            .compiled_method(Symbol::intern("run:"))
            .expect("run: is compiled")
            .code
            .clone()
    });
    let mut pending = vec![code];
    let mut sends = 0;
    while let Some(body) = pending.pop() {
        assert_eq!(body.family_size, 0);
        for instruction in &body.instructions() {
            if let Instruction::MakeClosure { code: block, .. } = instruction {
                pending.push(block.clone());
            }
            if let Some(site) = instruction.call_site() {
                assert_eq!(site, CallSite::UNCACHED);
                sends += 1;
            }
        }
    }
    assert_eq!(sends, 3, "do:, printString and size");
}

fn collect_ns_files(directory: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory)
        .expect("read corpus directory")
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            collect_ns_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "ns") {
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
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/modules");
    let mut files = Vec::new();
    collect_ns_files(&directory, &mut files);
    let mut compiled = 0;
    for path in &files {
        let source = std::fs::read_to_string(path).expect("read module");
        // Every corpus module parses (`crates/ns-syntax/tests/corpus.rs`).
        let Ok(module) = ns_syntax::parse(&source) else {
            panic!("{} does not parse", path.display());
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
