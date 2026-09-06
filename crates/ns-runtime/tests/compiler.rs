//! `compile_module` / `compile_initializer` over a real loaded class.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::bytecode::{method_key, CallSite, CodeKind, Instr, Literal};
use ns_runtime::mixin::mixin_key;
use ns_runtime::{compiler, Binding, ClassRef, Interpreter, ObjectData, SourceOrigin, Symbol};
use ns_syntax::Span;

/// `class`'s own mixin identity -- the key `compile_module` files its
/// initializer under, now that compiled code is keyed by mixin rather than
/// by class object (applications of a mixin share code).
fn class_mixin_key(class: &ClassRef) -> usize {
    let borrowed = class.borrow();
    let ObjectData::Class(data) = &borrowed.data else {
        panic!("not a class")
    };
    mixin_key(&data.application.mixin)
}

fn load(src: &str) -> (Interpreter, ClassRef, Rc<HashMap<Span, Binding>>) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(src),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let bindings = resolved.bindings.clone();
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class, bindings)
}

#[test]
fn compile_module_compiles_every_loaded_method_and_the_initializer() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n  bar = ( ^2 )\n) : ()";
    let (_interp, class, bindings) = load(src);
    let module = compiler::compile_module(&class, &bindings);
    assert_eq!(module.methods.len(), 2);
    for compiled in module.methods.values() {
        assert_eq!(compiled.kind, CodeKind::Method);
        assert!(matches!(compiled.instrs.last(), Some(Instr::Return { .. })));
    }
    let init = module
        .initializers
        .get(&class_mixin_key(&class))
        .expect("root initializer compiled");
    assert_eq!(init.kind, CodeKind::Initializer);
    assert_eq!(
        init.instrs,
        vec![Instr::LoadSelf { dst: 0 }, Instr::ImplicitReturn { reg: 0 }]
    );
    assert!(Rc::ptr_eq(&module.bindings, &bindings));
}

#[test]
fn compiled_method_is_reachable_by_its_dispatch_key() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n) : ()";
    let (_interp, class, bindings) = load(src);
    let module = compiler::compile_module(&class, &bindings);
    let method = {
        let borrowed = class.borrow();
        let ObjectData::Class(data) = &borrowed.data else {
            panic!()
        };
        data.application
            .mixin
            .methods
            .get(&Symbol::intern("foo"))
            .unwrap()
            .clone()
    };
    assert!(module.methods.contains_key(&method_key(&method)));
}

#[test]
fn compile_initializer_lowers_slots_then_init_exprs_with_own_slot_indices() {
    let src = "Newspeak3\n'T'\nclass Foo x: a = Object new (\n  | p = a. q ::= 5. |\n  q := p.\n) (\n) : ()";
    let (_interp, class, bindings) = load(src);
    let init = compiler::compile_initializer(&class, &bindings);
    assert_eq!(init.kind, CodeKind::Initializer);
    assert_eq!(
        init.num_args, 1,
        "the factory parameter `a` is the one argument"
    );
    assert_eq!(
        init.instrs,
        vec![
            // `p = a.` -- `a` is register 0, written straight into own
            // slot 0 (`vm::run_code` adds the frame's `slot_base` at run
            // time to get the absolute field index).
            Instr::InitSlot {
                src: 0,
                own_index: 0
            },
            // `q ::= 5.`
            Instr::LoadLiteral {
                dst: 1,
                literal: Literal::SmallInt(5)
            },
            Instr::InitSlot {
                src: 1,
                own_index: 1
            },
            // `q := p.` -- slot reads/writes are lexical sends, as in M1.
            Instr::SendLexical {
                depth: 0,
                selector: Symbol::intern("p"),
                arg_regs: vec![],
                dst: 1,
                call_site: CallSite { call_site_id: 3 },
            },
            Instr::SendLexical {
                depth: 0,
                selector: Symbol::intern("q:"),
                arg_regs: vec![1],
                dst: 2,
                call_site: CallSite { call_site_id: 4 },
            },
            Instr::LoadSelf { dst: 3 },
            Instr::ImplicitReturn { reg: 3 },
        ]
    );
    assert_eq!(init.statement_pc, vec![(0, 0..1), (1, 1..3), (2, 3..5)]);
    assert_eq!(init.num_registers, 4);
}

#[test]
fn an_uninitialized_slot_emits_nothing() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n  | a. b = 1. |\n) (\n) : ()";
    let (_interp, class, bindings) = load(src);
    let init = compiler::compile_initializer(&class, &bindings);
    let init_slots: Vec<u32> = init
        .instrs
        .iter()
        .filter_map(|i| match i {
            Instr::InitSlot { own_index, .. } => Some(*own_index),
            _ => None,
        })
        .collect();
    assert_eq!(init_slots, vec![1], "only `b` (field 1) has an initializer");
    assert_eq!(init.statement_pc.len(), 1);
}

/// The brief's original test for this task instantiated `Outer` twice and
/// compared the nested class's compiled code across instances -- but that
/// needs the nested-class accessor (`innerClass`), which does not exist yet
/// (this task was moved ahead of it, in the ORDERING RULING, specifically
/// because keying by class object -- one per enclosing instance -- would
/// have made the bytecode engine unable to find a freshly materialized
/// nested class's code at all). What is testable now, and is the actual
/// defect this task fixes, is that `compile_module` visits the *whole*
/// mixin tree -- nested declarations and class sides, not only the root's
/// own method table.
const NESTED_SRC: &str = "\
Newspeak3
'Test'
class Outer usingPlatform: p = Object new () (
    public class Inner = Object new (| v. |) (
        public value = ( ^v )
    )
    public classSideMarker = ( ^1 )
) : (
    public marker = ( ^2 )
)
";

#[test]
fn compile_module_compiles_every_mixin_in_the_tree_not_just_the_root() {
    let (_interp, class, bindings) = load(NESTED_SRC);
    let module = compiler::compile_module(&class, &bindings);

    let borrowed = class.borrow();
    let ObjectData::Class(data) = &borrowed.data else {
        panic!("expected a class object");
    };
    let mixin = &data.application.mixin;

    // The nested mixin's own method must be compiled -- proof the compiler
    // walked past the root's own method table into `nested`.
    let inner = &mixin.nested[0];
    let inner_value = inner
        .methods
        .get(&Symbol::intern("value"))
        .expect("Inner declares `value`");
    assert!(
        module.methods.contains_key(&method_key(inner_value)),
        "compile_module must visit a nested mixin's own methods"
    );

    // The root's class side is a mixin too, and must be walked the same way.
    let class_side = mixin.class_side.as_ref().expect("Outer has a class side");
    let marker = class_side
        .methods
        .get(&Symbol::intern("marker"))
        .expect("class side declares `marker`");
    assert!(
        module.methods.contains_key(&method_key(marker)),
        "compile_module must visit the class side's own methods"
    );

    // Every mixin visited (root, Inner, class side) gets its own compiled
    // initializer, keyed by its own mixin identity.
    assert!(module.initializers.contains_key(&mixin_key(inner)));
    assert!(module.initializers.contains_key(&mixin_key(class_side)));
}
