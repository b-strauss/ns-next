//! What `load_module` compiles onto the declarations it builds.

use indoc::indoc;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::bytecode::{CallSite, CodeKind, Instruction, Literal};
use ns_runtime::dispatch::Lookup;
use ns_runtime::{Interpreter, Mixin, Mutator, Root, SourceOrigin, Symbol, ValueKind};

fn load(source: &str) -> (Interpreter, Root) {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(source),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interpreter = Interpreter::bootstrap();
    let class = interpreter.load_module(&resolved);
    (interpreter, class)
}

/// The object `root` holds, for the heap accessors that take one. Every
/// caller is already inside an entry.
fn object<'gc>(mutator: &Mutator<'gc>, root: &Root) -> ns_runtime::ObjectReference<'gc> {
    match mutator.fetch(root).kind() {
        ValueKind::Object(object) => object,
        _ => panic!("expected an object"),
    }
}

fn root_mixin(interpreter: &mut Interpreter, class: &Root) -> Rc<Mixin> {
    interpreter.enter(|mutator| {
        mutator
            .heap()
            .class_application(object(mutator, class))
            .unwrap_or_else(|| panic!("not a class"))
            .mixin
            .clone()
    })
}

#[test]
fn loading_compiles_every_method_and_the_initializer_onto_the_mixin() {
    let source = indoc! {"
        class X = Object new (
        ) (
          foo = ( ^1 )
          bar = ( ^2 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let mixin = root_mixin(&mut interpreter, &class);
    assert_eq!(mixin.methods.len(), 2);
    for method in mixin.compiled_methods() {
        assert_eq!(method.code.kind, CodeKind::Method);
        assert!(matches!(
            method.code.instructions().last(),
            Some(Instruction::Return { .. })
        ));
    }
    let initializer = mixin
        .initializer_code
        .as_ref()
        .expect("root initializer compiled");
    assert_eq!(initializer.kind, CodeKind::Initializer);
    assert_eq!(
        initializer.instructions(),
        vec![
            Instruction::LoadSelf { destination: 0 },
            Instruction::ImplicitReturn { register: 0 }
        ]
    );
}

#[test]
fn the_method_dispatch_finds_carries_its_body() {
    let source = indoc! {"
        class X = Object new (
        ) (
          foo = ( ^1 )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    // Access-blind: the fixture's unmarked `foo` is protected, which
    // `lookup_public` would not find.
    interpreter.enter(|mutator| {
        match ns_runtime::dispatch::lookup_defined_for(
            mutator.heap(),
            object(mutator, &class),
            Symbol::intern("foo"),
        ) {
            Lookup::Method(method, _) => assert_eq!(method.code.kind, CodeKind::Method),
            _ => panic!("expected a method"),
        }
    });
}

#[test]
fn the_initializer_lowers_slots_then_initializer_expressions_with_own_slot_indices() {
    let source = indoc! {"
        class Foo x: a = Object new (
          | p = a. q ::= 5. |
          q:: p.
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let initializer = root_mixin(&mut interpreter, &class)
        .initializer_code
        .clone()
        .expect("an initializer");
    assert_eq!(initializer.kind, CodeKind::Initializer);
    assert_eq!(
        initializer.argument_count, 1,
        "the factory parameter `a` is the one argument"
    );
    assert_eq!(
        initializer.instructions(),
        vec![
            // `p = a.` -- `a` is register 0, written straight into own
            // slot 0 (the VM adds the activation's slot base at run
            // time to get the absolute field index).
            Instruction::InitializeSlot {
                source: 0,
                own_index: 0
            },
            // `q ::= 5.`
            Instruction::LoadLiteral {
                destination: 1,
                literal: Literal::SmallInteger(5)
            },
            Instruction::InitializeSlot {
                source: 1,
                own_index: 1
            },
            // `q:: p.` -- slot reads/writes are lexical sends.
            Instruction::SendLexical {
                depth: 0,
                selector: Symbol::intern("p"),
                argument_registers: vec![],
                destination: 1,
                call_site: CallSite { number: 0 },
            },
            Instruction::SendLexical {
                depth: 0,
                selector: Symbol::intern("q:"),
                argument_registers: vec![1],
                destination: 2,
                call_site: CallSite { number: 1 },
            },
            Instruction::LoadSelf { destination: 3 },
            Instruction::ImplicitReturn { register: 3 },
        ]
    );
    assert_eq!(
        initializer.statement_program_counter,
        vec![(0, 0..1), (1, 1..3), (2, 3..5)]
    );
    assert_eq!(initializer.register_count, 4);
}

#[test]
fn an_uninitialized_slot_emits_nothing() {
    let source = indoc! {"
        class X = Object new (
          | a. b = 1. |
        ) (
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let initializer = root_mixin(&mut interpreter, &class)
        .initializer_code
        .clone()
        .expect("an initializer");
    let initialize_slots: Vec<u32> = initializer
        .instructions()
        .iter()
        .filter_map(|instruction| match instruction {
            Instruction::InitializeSlot { own_index, .. } => Some(*own_index),
            _ => None,
        })
        .collect();
    assert_eq!(
        initialize_slots,
        vec![1],
        "only `b` (field 1) has an initializer"
    );
    assert_eq!(initializer.statement_program_counter.len(), 1);
}

/// The property that matters: loading compiles the *whole* mixin tree --
/// nested declarations and class sides, not only the root's own method
/// table.
const NESTED_SOURCE: &str = indoc! {"
    class Outer usingPlatform: p = Object new () (
        public class Inner = Object new (| v. |) (
            public value = ( ^v )
        )
        public classSideMarker = ( ^1 )
    ) : (
        public marker = ( ^2 )
    )
"};

#[test]
fn every_mixin_in_the_tree_gets_its_bodies() {
    let (mut interpreter, class) = load(NESTED_SOURCE);
    let mixin = root_mixin(&mut interpreter, &class);

    // The nested mixin's own method must be compiled -- proof the tree was
    // compiled past the root's own method table into `nested`.
    let inner = &interpreter.enter(|mutator| mutator.heap().nested_mixins(&mixin))[0];
    let inner_value = inner
        .compiled_method(Symbol::intern("value"))
        .expect("Inner declares `value`");
    assert_eq!(
        inner_value.code.kind,
        CodeKind::Method,
        "a nested mixin's own methods carry their bodies"
    );

    // The root's class side is a mixin too, and is compiled the same way.
    let class_side = mixin.class_side.as_ref().expect("Outer has a class side");
    let marker = class_side
        .compiled_method(Symbol::intern("marker"))
        .expect("class side declares `marker`");
    assert_eq!(
        marker.code.kind,
        CodeKind::Method,
        "the class side's own methods carry their bodies"
    );

    // Every mixin of the tree (root, Inner, class side) carries its own
    // compiled initializer.
    assert!(inner.initializer_code.is_some());
    assert!(class_side.initializer_code.is_some());
}

ns_runtime::for_each_lowering! {
    /// A lazy slot's getter (§6.3.3) answers a filled slot from inside its own
    /// activation: under both lowerings its body opens with
    /// `ReturnSlotIfFilled` at the slot's own index, before the initializer,
    /// and the method carries no fast path -- an arm for one would be inlined
    /// into every send site of the run loop (`docs/design/execution.md`, "What
    /// the platform module cost").
    fn a_lazy_slots_getter_opens_with_its_filled_test_and_has_no_fast_path(lowering) {
        let source = indoc! {"
            class X = Object new (
              | eager = 1. |
            ) (
              lazy later = 2.
            ) : ()
        "};
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = ns_runtime::resolve(
            module,
            Rc::from(source),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let class = interpreter.load_module(&resolved);
        let mixin = root_mixin(&mut interpreter, &class);
        let getter = mixin
            .compiled_method(Symbol::intern("later"))
            .unwrap_or_else(|| panic!("the lazy slot's getter is a compiled method"));
        assert_eq!(getter.fast_path, None);
        assert_eq!(
            getter.code.instructions().first(),
            Some(&Instruction::ReturnSlotIfFilled { own_index: 1 })
        );
        assert!(
            getter.code.instructions()[1..]
                .iter()
                .all(|instruction| !matches!(instruction, Instruction::ReturnSlotIfFilled { .. })),
            "the test runs once, first"
        );
    }
}
