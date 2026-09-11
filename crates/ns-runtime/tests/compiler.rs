//! What `load_module` compiles onto the declarations it builds.

use std::path::PathBuf;
use std::rc::Rc;

use ns_runtime::bytecode::{CallSite, CodeKind, Instr, Literal};
use ns_runtime::dispatch::Lookup;
use ns_runtime::{ClassRef, Interpreter, Mixin, SourceOrigin, Symbol};

fn load(src: &str) -> (Interpreter, ClassRef) {
    let module = ns_syntax::parse(src).expect("parse");
    let resolved = ns_runtime::resolve(
        module,
        Rc::from(src),
        SourceOrigin::File(PathBuf::from("t.ns")),
    )
    .expect("resolve");
    let mut interp = Interpreter::bootstrap();
    let class = interp.load_module(&resolved);
    (interp, class)
}

fn root_mixin(interp: &Interpreter, class: ClassRef) -> Rc<Mixin> {
    interp
        .heap_ref()
        .class_application(class)
        .unwrap_or_else(|| panic!("not a class"))
        .mixin
        .clone()
}

#[test]
fn loading_compiles_every_method_and_the_initializer_onto_the_mixin() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n  bar = ( ^2 )\n) : ()";
    let (interp, class) = load(src);
    let mixin = root_mixin(&interp, class);
    assert_eq!(mixin.methods.len(), 2);
    for method in mixin.methods.values() {
        assert_eq!(method.code.kind, CodeKind::Method);
        assert!(matches!(
            method.code.instrs.last(),
            Some(Instr::Return { .. })
        ));
    }
    let init = mixin
        .initializer_code
        .as_ref()
        .expect("root initializer compiled");
    assert_eq!(init.kind, CodeKind::Initializer);
    assert_eq!(
        init.instrs,
        vec![Instr::LoadSelf { dst: 0 }, Instr::ImplicitReturn { reg: 0 }]
    );
}

#[test]
fn the_method_dispatch_finds_carries_its_body() {
    let src = "Newspeak3\n'T'\nclass X = Object new (\n) (\n  foo = ( ^1 )\n) : ()";
    let (interp, class) = load(src);
    // Access-blind: the fixture's unmarked `foo` is protected, which
    // `lookup_public` would not find.
    match ns_runtime::dispatch::lookup_defined_for(interp.heap_ref(), class, Symbol::intern("foo"))
    {
        Lookup::Method(method, _) => assert_eq!(method.code.kind, CodeKind::Method),
        _ => panic!("expected a method"),
    }
}

#[test]
fn the_initializer_lowers_slots_then_init_exprs_with_own_slot_indices() {
    let src = "Newspeak3\n'T'\nclass Foo x: a = Object new (\n  | p = a. q ::= 5. |\n  q := p.\n) (\n) : ()";
    let (interp, class) = load(src);
    let init = root_mixin(&interp, class)
        .initializer_code
        .clone()
        .expect("an initializer");
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
    let (interp, class) = load(src);
    let init = root_mixin(&interp, class)
        .initializer_code
        .clone()
        .expect("an initializer");
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

/// The property that matters: loading compiles the *whole* mixin tree --
/// nested declarations and class sides, not only the root's own method
/// table.
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
fn every_mixin_in_the_tree_gets_its_bodies() {
    let (interp, class) = load(NESTED_SRC);
    let mixin = root_mixin(&interp, class);

    // The nested mixin's own method must be compiled -- proof the tree was
    // compiled past the root's own method table into `nested`.
    let inner = &mixin.nested[0];
    let inner_value = inner
        .methods
        .get(&Symbol::intern("value"))
        .expect("Inner declares `value`");
    assert_eq!(
        inner_value.code.kind,
        CodeKind::Method,
        "a nested mixin's own methods carry their bodies"
    );

    // The root's class side is a mixin too, and is compiled the same way.
    let class_side = mixin.class_side.as_ref().expect("Outer has a class side");
    let marker = class_side
        .methods
        .get(&Symbol::intern("marker"))
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
