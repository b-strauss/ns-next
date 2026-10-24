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
        .own_initializer_code
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
        .own_initializer_code
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
                argument_base: 1,
                argument_sources: vec![],
                destination: 1,
                call_site: CallSite { number: 0 },
            },
            Instruction::SendLexical {
                depth: 0,
                selector: Symbol::intern("q:"),
                argument_base: 2,
                argument_sources: vec![1],
                destination: 3,
                call_site: CallSite { number: 1 },
            },
            Instruction::LoadSelf { destination: 4 },
            Instruction::ImplicitReturn { register: 4 },
        ]
    );
    assert_eq!(
        initializer.statement_program_counter,
        vec![(0, 0..1), (1, 1..3), (2, 3..5)]
    );
    assert_eq!(initializer.register_count, 5);
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
        .own_initializer_code
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

/// The mixin of the class `name` that the module whose root `class` is
/// declares directly.
fn nested_mixin(interpreter: &mut Interpreter, class: &Root, name: &str) -> Rc<Mixin> {
    let root = root_mixin(interpreter, class);
    interpreter
        .enter(|mutator| mutator.heap().nested_mixins(&root))
        .into_iter()
        .find(|nested| nested.name.as_str() == name)
        .unwrap_or_else(|| panic!("the module declares {name}"))
}

/// A declaration whose superclass clause passes an argument computed from
/// its own factory parameter.
const SUPERCLASS_WITH_ARGUMENT: &str = indoc! {"
    class Outer = Object new () (
      class A y: value = Object new () ()
      class B x: x = A y: x + 1 (| s = x. |) ()
    ) : ()
"};

/// A declaration with two `<:` clauses, the second passing an argument.
const TWO_APPLIED_CLAUSES: &str = indoc! {"
    class Outer = Object new () (
      class S = Object new () ()
      class M1 = Object new () ()
      class M2 tag: value = Object new () ()
      class C = S new <: M1 new <: M2 tag: 1 () ()
    ) : ()
"};

/// §6.3: the superclass clause's message is evaluated in the factory's
/// scope and the superclass's initializer runs with it before any of the
/// class's own slot initializers.
#[test]
fn a_full_initializer_initializes_its_superclass_before_its_own_slots() {
    let (mut interpreter, class) = load(SUPERCLASS_WITH_ARGUMENT);
    let full = nested_mixin(&mut interpreter, &class, "B")
        .initializer_code
        .clone()
        .expect("a full initializer");
    assert_eq!(full.kind, CodeKind::Initializer);
    assert_eq!(full.argument_count, 1);
    let instructions = full.instructions();
    let sum = instructions
        .iter()
        .position(|instruction| matches!(instruction, Instruction::IntegerBinary { .. }))
        .expect("the `+` of `x + 1`");
    let Instruction::IntegerBinary {
        destination: sum_register,
        left: 0,
        ..
    } = instructions[sum]
    else {
        panic!("`x + 1` adds to the parameter `x`")
    };
    let superclass = instructions
        .iter()
        .position(|instruction| matches!(instruction, Instruction::InitializeSuperclass { .. }))
        .expect("the superclass's initialization");
    assert!(sum < superclass);
    assert!(matches!(
        &instructions[superclass],
        Instruction::InitializeSuperclass { steps: 1, selector, argument_sources, .. }
            if *selector == Symbol::intern("y:") && *argument_sources == vec![sum_register]
    ));
    let slot = instructions
        .iter()
        .position(|instruction| matches!(instruction, Instruction::InitializeSlot { .. }))
        .expect("the slot `s`");
    assert!(superclass < slot);
    assert_eq!(
        instructions[slot],
        Instruction::InitializeSlot {
            source: 0,
            own_index: 0
        }
    );
    let [.., load_self, implicit_return] = &instructions[..] else {
        panic!("the body has at least two instructions")
    };
    let Instruction::LoadSelf { destination } = *load_self else {
        panic!("the body answers self")
    };
    assert_eq!(
        *implicit_return,
        Instruction::ImplicitReturn {
            register: destination
        }
    );
}

/// §6.1's check of each `<:` clause precedes everything that runs, the top
/// clause's first, as the walk checked them on its way up; then the
/// superclass, then each applied level from the lowest, its message's
/// arguments evaluated just before it (§6.2).
#[test]
fn a_full_initializer_checks_every_applied_clause_before_the_superclass_message() {
    let (mut interpreter, class) = load(TWO_APPLIED_CLAUSES);
    let full = nested_mixin(&mut interpreter, &class, "C")
        .initializer_code
        .clone()
        .expect("a full initializer");
    let instructions = full.instructions();
    assert_eq!(instructions.len(), 8);
    assert_eq!(
        instructions[..2],
        [
            Instruction::CheckAppliedClause {
                steps: 1,
                selector: Symbol::intern("tag:")
            },
            Instruction::CheckAppliedClause {
                steps: 2,
                selector: Symbol::NEW
            },
        ]
    );
    assert!(matches!(
        &instructions[2],
        Instruction::InitializeSuperclass { steps: 3, selector, argument_sources, .. }
            if *selector == Symbol::NEW && argument_sources.is_empty()
    ));
    assert!(matches!(
        &instructions[3],
        Instruction::InitializeAppliedLevel { steps: 2, argument_sources, .. }
            if argument_sources.is_empty()
    ));
    let Instruction::LoadLiteral {
        destination: tag,
        literal: Literal::SmallInteger(1),
    } = instructions[4]
    else {
        panic!("the literal 1 of `M2 tag: 1`")
    };
    assert!(matches!(
        &instructions[5],
        Instruction::InitializeAppliedLevel { steps: 1, argument_sources, .. }
            if *argument_sources == vec![tag]
    ));
    assert!(matches!(instructions[6], Instruction::LoadSelf { .. }));
    assert!(matches!(
        instructions[7],
        Instruction::ImplicitReturn { .. }
    ));
}

/// The own part is exactly the body every mixin compiled before the
/// initializers took in their clauses. The expectations were read off
/// `disasm --initializer` of `vm_initializer.rs`'s three-level chain before
/// the change, not derived from the compiler that now emits them.
#[test]
fn the_own_part_is_the_body_of_today() {
    let source = indoc! {"
        class Levels = Object new () (
            public class Base withBase: b = Object new (| b1 ::= b. b2 ::= 10. |) (
            )
            public class Middle withMiddle: m = Base withBase: m + 1 (| m1 ::= m. m2 ::= 20. |) (
            )
            public class Leaf = Middle withMiddle: 2 (| l1 ::= 3. l2 ::= 30. |) (
            )
        ) : ()
    "};
    let (mut interpreter, class) = load(source);
    let literal = |destination: u16, value: i64| Instruction::LoadLiteral {
        destination,
        literal: Literal::SmallInteger(value),
    };
    let slot = |source: u16, own_index: u32| Instruction::InitializeSlot { source, own_index };
    let answer = |register: u16| {
        [
            Instruction::LoadSelf {
                destination: register,
            },
            Instruction::ImplicitReturn { register },
        ]
    };
    let parameter_then = |second: i64| {
        let mut instructions = vec![slot(0, 0), literal(1, second), slot(1, 1)];
        instructions.extend(answer(2));
        instructions
    };
    let mut leaf = vec![literal(0, 3), slot(0, 0), literal(0, 30), slot(0, 1)];
    leaf.extend(answer(1));
    let expected = [
        ("Base", parameter_then(10), 3, vec![(0, 0..1), (1, 1..3)]),
        ("Middle", parameter_then(20), 3, vec![(0, 0..1), (1, 1..3)]),
        ("Leaf", leaf, 2, vec![(0, 0..2), (1, 2..4)]),
    ];
    for (name, instructions, register_count, statements) in expected {
        let own = nested_mixin(&mut interpreter, &class, name)
            .own_initializer_code
            .clone()
            .expect("an own part");
        assert_eq!(own.kind, CodeKind::Initializer, "{name}");
        assert_eq!(own.instructions(), instructions, "{name}");
        assert_eq!(own.register_count, register_count, "{name}");
        assert_eq!(own.statement_program_counter, statements, "{name}");
    }
}

/// A level a `<:` clause added and reached from outside runs its mixin's
/// superclass clause and own part, never the declaration's `<:` clauses;
/// a mixin without clauses has nothing to leave out, so it has no such
/// body.
#[test]
fn a_standalone_initializer_exists_only_for_a_declaration_with_clauses() {
    let (mut interpreter, class) = load(TWO_APPLIED_CLAUSES);
    let applying = nested_mixin(&mut interpreter, &class, "C");
    let standalone = applying
        .standalone_initializer_code
        .clone()
        .expect("C writes `<:` clauses");
    assert_eq!(standalone.kind, CodeKind::Initializer);
    let instructions = standalone.instructions();
    assert!(instructions.iter().all(|instruction| !matches!(
        instruction,
        Instruction::CheckAppliedClause { .. } | Instruction::InitializeAppliedLevel { .. }
    )));
    assert!(matches!(
        &instructions[0],
        Instruction::InitializeSuperclass { steps: 1, selector, argument_sources, .. }
            if *selector == Symbol::NEW && argument_sources.is_empty()
    ));
    for name in ["S", "M1", "M2"] {
        let mixin = nested_mixin(&mut interpreter, &class, name);
        assert!(mixin.standalone_initializer_code.is_none(), "{name}");
        assert!(mixin.initializer_code.is_some(), "{name}");
    }
    let root = root_mixin(&mut interpreter, &class);
    assert!(root.standalone_initializer_code.is_none());
    let class_side = root.class_side.as_ref().expect("Outer has a class side");
    assert!(class_side.standalone_initializer_code.is_none());
}

/// A factory allocates an instance of the class it was sent to, runs that
/// class's initializer on it with the factory's arguments, and answers the
/// instance, whatever the initializer answers.
#[test]
fn the_factory_body_allocates_initializes_and_returns_the_instance() {
    let body = ns_runtime::compiler::factory_body(2);
    assert_eq!(body.kind, CodeKind::Method);
    assert_eq!(body.argument_count, 2);
    assert_eq!(body.register_count, 4);
    assert_eq!(
        body.instructions(),
        vec![
            Instruction::AllocateInstance { destination: 2 },
            Instruction::InitializeInstance {
                instance: 2,
                destination: 3,
            },
            Instruction::Return { register: 2 },
        ]
    );
    // An arity read off an image cannot make the body grow with it.
    let widest = ns_runtime::compiler::factory_body(u16::MAX - 2);
    assert_eq!(widest.instructions().len(), 3);
    assert_eq!(widest.register_count, u16::MAX);
}

/// A clause argument is lowered inside the initializer, so a block in one
/// that reads a factory parameter must be seen by the capture analysis: the
/// optimized lowering hands the block the parameter's register as a copied
/// value, the reference lowering reads it by reference, and neither body
/// is too large.
#[test]
fn a_block_in_a_clause_argument_captures_the_factory_parameter() {
    let source = indoc! {"
        class Outer = Object new () (
          class A y: value = Object new () ()
          class B x: x = A y: [x] value (| s = x. |) ()
        ) : ()
    "};
    for lowering in [
        ns_runtime::Lowering::Optimized,
        ns_runtime::Lowering::Reference,
    ] {
        let module = ns_syntax::parse(source).expect("parse");
        let resolved = ns_runtime::resolve(
            module,
            Rc::from(source),
            SourceOrigin::File(PathBuf::from("t.ns")),
        )
        .expect("resolve");
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        let class = interpreter.load_module(&resolved);
        let full = nested_mixin(&mut interpreter, &class, "B")
            .initializer_code
            .clone()
            .expect("a full initializer");
        let instructions = full.instructions();
        assert!(
            instructions.iter().all(|instruction| !matches!(
                instruction,
                Instruction::LoadLiteral {
                    literal: Literal::BodyTooLarge,
                    ..
                }
            )),
            "{lowering:?}: the body is not too large"
        );
        let (block, copied) = instructions
            .iter()
            .find_map(|instruction| match instruction {
                Instruction::MakeClosure { code, copied, .. } => {
                    Some((code.clone(), copied.clone()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{lowering:?}: the block of `[x] value`"));
        let superclass = instructions
            .iter()
            .position(|instruction| matches!(instruction, Instruction::InitializeSuperclass { .. }))
            .expect("the superclass's initialization");
        let closure = instructions
            .iter()
            .position(|instruction| matches!(instruction, Instruction::MakeClosure { .. }))
            .expect("the closure");
        assert!(
            closure < superclass,
            "{lowering:?}: the argument precedes its use"
        );
        let block_instructions = block.instructions();
        match lowering {
            ns_runtime::Lowering::Optimized => {
                assert_eq!(copied, vec![0], "the parameter `x` is handed over");
                assert_eq!(block.copied_count, 1);
                assert_eq!(block.outer_reach, 0);
            }
            ns_runtime::Lowering::Reference => {
                assert!(copied.is_empty());
                assert!(
                    block_instructions.iter().any(|instruction| matches!(
                        instruction,
                        Instruction::LoadOuterLocal {
                            depth: 1,
                            index: 0,
                            ..
                        }
                    )),
                    "the block reads `x` in the initializer's frame"
                );
                assert_eq!(block.outer_reach, 1, "the frame keeps its registers for it");
            }
        }
    }
}
