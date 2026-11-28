use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use indoc::{formatdoc, indoc};

use super::again::{BodyOrigin, Member, PairedBodies, compile_level_again};
use super::{
    BodyMetadata, Cache, MetadataCollector, SendEntry, VariableEntry, VariablePlace, VectorHolder,
};
use crate::bytecode::{CompiledCode, HEADER_REGISTERS, Instruction, compiles_alike, may_have_run};
use crate::compiler::Lowering;
use crate::interpreter::Mutator;
use crate::mixin::Mixin;
use crate::{Interpreter, Root, Symbol};

/// The module `source` resolved, and its root class's first instance-side
/// method compiled under `lowering` with `collector`, if any.
fn compile(
    source: &str,
    lowering: Lowering,
    collector: Option<&MetadataCollector>,
) -> CompiledCode {
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = match crate::resolve(
        module,
        Rc::from(source),
        crate::SourceOrigin::File(PathBuf::from("t.ns")),
    ) {
        Ok(resolved) => resolved,
        Err(_) => panic!("does not resolve"),
    };
    let method = resolved
        .module
        .root_class
        .instance_side
        .methods()
        .next()
        .expect("a method");
    crate::compiler::compile_method(method, &resolved.bindings, lowering, collector)
}

/// The first method of `source` compiled under the optimized lowering, and
/// what the collector recorded for it.
fn collected(source: &str) -> (CompiledCode, BodyMetadata, MetadataCollector) {
    let collector = MetadataCollector::new();
    let code = compile(source, Lowering::Optimized, Some(&collector));
    let metadata = collector.take(&code).expect("the method's metadata");
    (code, metadata, collector)
}

/// The text of each statement start, in order.
fn statement_texts<'text>(metadata: &BodyMetadata, source: &'text str) -> Vec<&'text str> {
    metadata
        .statements
        .iter()
        .map(|entry| entry.span.text(source))
        .collect()
}

/// A send entry's receiver and selector as text.
fn send_texts<'text>(
    entry: &SendEntry,
    source: &'text str,
) -> (Option<&'text str>, Vec<&'text str>) {
    (
        entry.receiver.map(|span| span.text(source)),
        entry
            .selector
            .iter()
            .map(|span| span.text(source))
            .collect(),
    )
}

/// The `n`-th statement range of `code`'s `statement_program_counter`, as
/// a statement entry's range is written.
fn statement_range(code: &CompiledCode, position: usize) -> std::ops::Range<u32> {
    let range = &code.statement_program_counter[position].1;
    u32::try_from(range.start).expect("small")..u32::try_from(range.end).expect("small")
}

/// The first block `code` creates.
fn first_block(code: &CompiledCode) -> Rc<CompiledCode> {
    code.instructions()
        .into_iter()
        .find_map(|instruction| match instruction {
            Instruction::MakeClosure { code, .. } => Some(code),
            _ => None,
        })
        .expect("a block")
}

fn variable(name: &str, depth: u16, place: VariablePlace) -> VariableEntry {
    VariableEntry {
        name: Symbol::intern(name),
        assignable: true,
        depth,
        place,
        copied_by: Box::new([]),
    }
}

/// `variable`, for a parameter, which no assignment writes.
fn parameter(name: &str, depth: u16, place: VariablePlace) -> VariableEntry {
    VariableEntry {
        assignable: false,
        ..variable(name, depth, place)
    }
}

/// `entry`, copied by value by the `MakeClosure`s at `indices`.
fn copied_by(entry: VariableEntry, indices: &[u32]) -> VariableEntry {
    VariableEntry {
        copied_by: indices.into(),
        ..entry
    }
}

/// The index of every `MakeClosure` of `code`, in order.
fn closure_indices(code: &CompiledCode) -> Vec<u32> {
    code.instructions()
        .iter()
        .enumerate()
        .filter(|(_, instruction)| matches!(instruction, Instruction::MakeClosure { .. }))
        .map(|(index, _)| u32::try_from(index).expect("small"))
        .collect()
}

/// The variable `name` of `metadata`'s table.
fn variable_named<'metadata>(
    metadata: &'metadata BodyMetadata,
    name: &str,
) -> &'metadata VariableEntry {
    metadata
        .variables
        .iter()
        .find(|entry| entry.name == Symbol::intern(name))
        .expect("the variable is named")
}

/// The first instruction of the statement of `metadata` at `position`.
fn statement_start(metadata: &BodyMetadata, position: usize) -> u32 {
    metadata.statements[position].instructions.start
}

#[test]
fn copied_by_names_the_capturing_closures() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m = ( | x | x:: 1. holder value: [ x ]. sink: x )
          holder = ( ^nil )
          sink: value = ( )
        ) : ()
    "};
    let (code, metadata, _) = collected(source);
    let closures = closure_indices(&code);
    assert_eq!(closures.len(), 1);
    assert_eq!(*variable_named(&metadata, "x").copied_by, closures[..]);

    let (reference, reference_collector) = collected_under(source, Lowering::Reference);
    let reference_metadata = reference_collector
        .take(&reference)
        .expect("the reference method's metadata");
    assert!(
        variable_named(&reference_metadata, "x")
            .copied_by
            .is_empty(),
        "the reference lowering copies nothing"
    );
}

#[test]
fn copied_by_is_empty_for_a_parameter_an_immutable_local_and_a_vector_variable() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m: p = ( | i = 1. v | v:: 2. holder value: [ p + i + v. v:: 3 ] )
          holder = ( ^nil )
        ) : ()
    "};
    let (code, metadata, _) = collected(source);
    let Some(Instruction::MakeClosure { copied, .. }) = code
        .instructions()
        .into_iter()
        .find(|instruction| matches!(instruction, Instruction::MakeClosure { .. }))
    else {
        panic!("a block");
    };
    // The parameter and the immutable local are handed by value, the
    // vector variable through its vector.
    assert!(copied.contains(&0), "the parameter is handed");
    for name in ["p", "i", "v"] {
        assert!(
            variable_named(&metadata, name).copied_by.is_empty(),
            "{name}"
        );
    }
}

#[test]
fn may_have_run_follows_the_back_edge() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m = ( | x i | x:: 1. i:: 0. [ i < 3 ] whileTrue: [ sink: x. holder value: [ x ]. i:: i + 1 ] )
          holder = ( ^nil )
          sink: value = ( )
        ) : ()
    "};
    let (code, metadata, _) = collected(source);
    assert_eq!(
        statement_texts(&metadata, source),
        vec![
            "x:: 1",
            "i:: 0",
            "[ i < 3 ] whileTrue: [ sink: x. holder value: [ x ]. i:: i + 1 ]",
            "i < 3",
            "sink: x",
            "holder value: [ x ]",
            "i:: i + 1"
        ]
    );
    let copies = variable_named(&metadata, "x").copied_by.clone();
    assert_eq!(copies.len(), 1);
    for before_the_loop in 0..3 {
        assert!(!may_have_run(
            &code,
            &copies,
            statement_start(&metadata, before_the_loop)
        ));
    }
    // An earlier iteration made the closure before this one reached the
    // loop's condition and its first statement.
    for in_the_loop in 3..7 {
        assert!(may_have_run(
            &code,
            &copies,
            statement_start(&metadata, in_the_loop)
        ));
    }
}

#[test]
fn may_have_run_is_false_before_the_capture() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m = ( | x | x:: 1. sink: x. holder value: [ x ]. sink: x )
          holder = ( ^nil )
          sink: value = ( )
        ) : ()
    "};
    let (code, metadata, _) = collected(source);
    assert_eq!(
        statement_texts(&metadata, source),
        vec!["x:: 1", "sink: x", "holder value: [ x ]", "sink: x"]
    );
    let copies = variable_named(&metadata, "x").copied_by.clone();
    assert_eq!(copies.len(), 1);
    assert!(!may_have_run(&code, &copies, statement_start(&metadata, 1)));
    // The statement that makes the closure has not made it at its start.
    assert!(!may_have_run(&code, &copies, statement_start(&metadata, 2)));
    assert!(may_have_run(&code, &copies, statement_start(&metadata, 3)));
    assert!(!may_have_run(&code, &[], statement_start(&metadata, 3)));
}

#[test]
fn every_statement_start_is_recorded_with_its_span() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m = ( | a = 1. | a foo. ^a )
        ) : ()
    "};
    let (code, metadata, _) = collected(source);
    assert_eq!(
        statement_texts(&metadata, source),
        vec!["a = 1.", "a foo", "^a"]
    );
    let length = u32::try_from(code.instruction_count()).expect("small");
    for (position, entry) in metadata.statements.iter().enumerate() {
        assert!(entry.instructions.end <= length);
        assert_eq!(entry.instructions, statement_range(&code, position));
    }
}

#[test]
fn an_inlined_block_s_statements_are_statement_starts() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m: x = ( x ifTrue: [ 1 foo. 2 bar ]. ^3 )
        ) : ()
    "};
    let (code, metadata, _) = collected(source);
    assert_eq!(
        statement_texts(&metadata, source),
        vec!["x ifTrue: [ 1 foo. 2 bar ]", "1 foo", "2 bar", "^3"]
    );
    let statements = &metadata.statements;
    assert_eq!(statements[0].instructions, statement_range(&code, 0));
    assert_eq!(statements[3].instructions, statement_range(&code, 1));
    for inner in &statements[1..3] {
        assert!(statements[0].instructions.start <= inner.instructions.start);
        assert!(inner.instructions.end <= statements[0].instructions.end);
    }
    assert!(statements[1].instructions.end <= statements[2].instructions.start);
}

#[test]
fn a_send_names_its_call_site_receiver_and_selector() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m: x = ( ^x at: 1 put: 2 )
        ) : ()
    "};
    let (code, metadata, _) = collected(source);
    assert_eq!(metadata.sends.len(), 1);
    let entry = &metadata.sends[0];
    let Instruction::SendPublic { call_site, .. } = code.instruction(entry.index as usize) else {
        panic!("the entry names a SendPublic");
    };
    assert_eq!(entry.call_site, Some(call_site));
    assert_eq!(send_texts(entry, source), (Some("x"), vec!["at:", "put:"]));
}

#[test]
fn an_implicit_unary_send_and_a_setter_name_the_identifier() {
    let source = indoc! {"
        class X = Object new (
        | s ::= 0. |
        ) (
          m = ( foo. s:: 3 )
          foo = ( )
        ) : ()
    "};
    let (code, metadata, _) = collected(source);
    assert_eq!(metadata.sends.len(), 2);
    for (entry, expected) in metadata.sends.iter().zip(["foo", "s:"]) {
        let Instruction::SendLexical {
            selector,
            call_site,
            ..
        } = code.instruction(entry.index as usize)
        else {
            panic!("the entry names a SendLexical");
        };
        assert_eq!(selector, Symbol::intern(expected));
        assert_eq!(entry.call_site, Some(call_site));
    }
    assert_eq!(send_texts(&metadata.sends[0], source), (None, vec!["foo"]));
    assert_eq!(send_texts(&metadata.sends[1], source), (None, vec!["s"]));
}

#[test]
fn a_cascade_s_sends_name_the_cascade_s_receiver() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m: x = ( x foo; bar: 1; baz )
        ) : ()
    "};
    let (_, metadata, _) = collected(source);
    let texts: Vec<_> = metadata
        .sends
        .iter()
        .map(|entry| send_texts(entry, source))
        .collect();
    assert_eq!(
        texts,
        vec![
            (Some("x"), vec!["foo"]),
            (Some("x"), vec!["bar:"]),
            (Some("x"), vec!["baz"]),
        ]
    );
}

#[test]
fn an_integer_fallback_and_a_conditional_fallback_are_sends() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m: x = ( x + 1. x ifTrue: [ 2 ] )
        ) : ()
    "};
    let (code, metadata, collector) = collected(source);
    let texts: Vec<_> = metadata
        .sends
        .iter()
        .map(|entry| send_texts(entry, source))
        .collect();
    assert_eq!(
        texts,
        vec![(Some("x"), vec!["+"]), (Some("x"), vec!["ifTrue:"])]
    );
    for entry in metadata.sends.iter() {
        let Instruction::SendPublic { call_site, .. } = code.instruction(entry.index as usize)
        else {
            panic!("the entry names a SendPublic");
        };
        assert_eq!(entry.call_site, Some(call_site));
    }
    // The fallback closure is a body of its own, with its own table.
    let fallback = first_block(&code);
    let fallback_metadata = collector.take(&fallback).expect("the fallback's metadata");
    assert_eq!(statement_texts(&fallback_metadata, source), vec!["2"]);
}

#[test]
fn a_block_s_variables_cover_its_whole_lexical_chain() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m: p = ( | t u | ^[:b | t + b + u. u:: 1 ] )
        ) : ()
    "};
    let (code, metadata, collector) = collected(source);
    // The method: `p`, then `t` and `u` after the header's numbers, and
    // the vector register after them; the block copies `t`, the one
    // assignable local it is handed by value.
    let t_register = 1 + HEADER_REGISTERS;
    let vector_register = t_register + 2;
    assert_eq!(
        metadata.variables.to_vec(),
        vec![
            parameter("p", 0, VariablePlace::Register(0)),
            copied_by(
                variable("t", 0, VariablePlace::Register(t_register)),
                &closure_indices(&code),
            ),
            variable(
                "u",
                0,
                VariablePlace::Vector {
                    vector: VectorHolder::Register(vector_register),
                    slot: 0
                }
            ),
        ]
    );
    let Some(Instruction::MakeClosure {
        code: block,
        copied,
        ..
    }) = code
        .instructions()
        .into_iter()
        .find(|instruction| matches!(instruction, Instruction::MakeClosure { .. }))
    else {
        panic!("a block");
    };
    // A value the block was handed sits in the register after its
    // parameter and the header's numbers that its position names.
    let handed = |register: u16| {
        let position = copied
            .iter()
            .position(|held| *held == register)
            .expect("handed to the block");
        1 + HEADER_REGISTERS + u16::try_from(position).expect("small")
    };
    let block_metadata = collector.take(&block).expect("the block's metadata");
    assert_eq!(
        block_metadata.variables.to_vec(),
        vec![
            parameter("b", 0, VariablePlace::Register(0)),
            parameter(
                "p",
                1,
                VariablePlace::Outer {
                    hops: 1,
                    register: 0
                }
            ),
            variable("t", 1, VariablePlace::Register(handed(t_register))),
            variable(
                "u",
                1,
                VariablePlace::Vector {
                    vector: VectorHolder::Register(handed(vector_register)),
                    slot: 0
                }
            ),
        ]
    );
}

#[test]
fn the_innermost_of_a_shadowed_name_comes_first() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m: t = ( ^[:t | t ] )
        ) : ()
    "};
    let (code, _, collector) = collected(source);
    let block = first_block(&code);
    let block_metadata = collector.take(&block).expect("the block's metadata");
    assert_eq!(
        block_metadata.variables.to_vec(),
        vec![
            parameter("t", 0, VariablePlace::Register(0)),
            parameter(
                "t",
                1,
                VariablePlace::Outer {
                    hops: 1,
                    register: 0
                }
            ),
        ]
    );
}

// spec: vm.compiler.compiling-again-changes-nothing
#[test]
fn compiling_without_a_collector_emits_the_same_code() {
    let source = indoc! {"
        class X = Object new (
        | s ::= 0. |
        ) (
          m: p = (
            | t u a = 1. |
            u:: p + 1.
            s:: t.
            p ifTrue: [ u:: 2 ] ifFalse: [ ^[:b | t + b + u. u:: b ] ].
            p foo; bar: [ a ].
            [ u < 3 ] whileTrue: [ u:: u + 1 ].
            ^u
          )
        ) : ()
    "};
    let collector = MetadataCollector::new();
    assert_eq!(
        compile(source, Lowering::Optimized, None),
        compile(source, Lowering::Optimized, Some(&collector))
    );
}

#[test]
fn a_cloned_body_s_tables_move_with_rekey() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m = ( ^1 )
        ) : ()
    "};
    let collector = MetadataCollector::new();
    let code = compile(source, Lowering::Optimized, Some(&collector));
    let clone = code.clone();
    collector.rekey(&code, &clone);
    assert!(collector.take(&code).is_none());
    assert!(collector.take(&clone).is_some());
}

/// A module whose two methods are past the compiler's argument limit,
/// the first holding a block nested in a block, each of which compiles
/// before the send that makes its creator too large.
fn too_large_methods() -> String {
    let arguments = "k: 1 ".repeat(256);
    formatdoc! {"
        class Over = Object new (
        ) (
          public first = ( | b | b:: [[3]]. ^self {arguments})
          public second = ( ^self {arguments})
        ) : ()
    "}
}

#[test]
fn a_too_large_body_leaves_no_tables_of_the_blocks_it_dropped() {
    let source = too_large_methods();
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let collector = MetadataCollector::new();
        let code = compile(&source, lowering, Some(&collector));
        assert!(
            matches!(
                code.instruction(0),
                Instruction::LoadLiteral {
                    literal: crate::bytecode::Literal::BodyTooLarge(_),
                    ..
                }
            ),
            "{lowering:?}: the method is too large"
        );
        // The blocks were recorded as they were finished, and dropped
        // with their creator: an entry left under their addresses would
        // be answered for the next body allocated there.
        assert!(collector.is_empty(), "{lowering:?}: tables were left");
    }
}

crate::for_each_lowering! {
    fn no_too_large_body_of_a_level_gets_tables(lowering) {
        let source = too_large_methods();
        let (mut interpreter, root) = loaded_under(&source, lowering);
        interpreter.enter(|mutator| {
            let mixin = applied_mixin(mutator, &root);
            let pairs = compile_level_again(mutator, &mixin, lowering).expect("the level");
            let paired_bodies = paired(&pairs);
            for selector in ["first", "second"] {
                let code = method_code(&mixin, selector);
                assert_eq!(
                    paired_bodies.get(&(Rc::as_ptr(&code) as usize)),
                    Some(&false),
                    "{selector} pairs, without tables"
                );
            }
        });
    }
}

/// The code `code` and every block it creates.
fn family(code: &Rc<CompiledCode>, into: &mut Vec<Rc<CompiledCode>>) {
    into.push(Rc::clone(code));
    for instruction in code.instructions() {
        if let Instruction::MakeClosure { code: block, .. } = instruction {
            family(&block, into);
        }
    }
}

#[test]
fn every_body_s_statement_ranges_are_statement_starts() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m: p = (
            | t u a = 1. |
            u:: p + 1.
            p ifTrue: [ u:: 2. t foo ] ifFalse: [ ^[:b | t + b + u. u:: b ] ].
            [ u < 3 ] whileTrue: [ u:: u + 1 ].
            ^[ a bar ]
          )
        ) : ()
    "};
    let collector = MetadataCollector::new();
    let code = Rc::new(compile(source, Lowering::Optimized, Some(&collector)));
    let mut bodies = Vec::new();
    family(&code, &mut bodies);
    assert!(
        bodies.len() >= 4,
        "the method, two blocks and two fallbacks"
    );
    for body in &bodies {
        let metadata = collector.take(body).expect("every body's metadata");
        let length = u32::try_from(body.instruction_count()).expect("small");
        for position in 0..body.statement_program_counter.len() {
            let range = statement_range(body, position);
            assert!(
                metadata
                    .statements
                    .iter()
                    .any(|entry| entry.instructions == range),
                "a statement start for every statement range"
            );
        }
        for entry in metadata.statements.iter() {
            assert!(entry.instructions.start <= entry.instructions.end);
            assert!(entry.instructions.end <= length);
        }
        for entry in metadata.sends.iter() {
            let instruction = body.instruction(entry.index as usize);
            assert_eq!(
                instruction.call_site(),
                entry.call_site,
                "a send entry names a send of exactly its call site"
            );
        }
    }
}

#[test]
fn a_variable_an_enclosing_block_was_handed_is_read_from_that_block() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m: p = ( | u | ^[ u:: p. [ 3 ] ] )
        ) : ()
    "};
    let (code, _, collector) = collected(source);
    let outer_block = first_block(&code);
    let Some(Instruction::MakeClosure { copied, .. }) = code
        .instructions()
        .into_iter()
        .find(|instruction| matches!(instruction, Instruction::MakeClosure { .. }))
    else {
        panic!("a block");
    };
    // The method keeps `p` in register 0, `u` past the header, and its
    // vector after `u`; the outer block, which takes no parameter, holds
    // what it was handed after the header's numbers.
    let vector_register = HEADER_REGISTERS + 2;
    let handed = |register: u16| {
        let position = copied
            .iter()
            .position(|held| *held == register)
            .expect("handed to the outer block");
        HEADER_REGISTERS + u16::try_from(position).expect("small")
    };
    let inner_block = first_block(&outer_block);
    let inner_metadata = collector
        .take(&inner_block)
        .expect("the inner block's metadata");
    assert_eq!(
        inner_metadata.variables.to_vec(),
        vec![
            parameter(
                "p",
                2,
                VariablePlace::Outer {
                    hops: 1,
                    register: handed(0)
                }
            ),
            variable(
                "u",
                2,
                VariablePlace::Vector {
                    vector: VectorHolder::Outer {
                        hops: 1,
                        register: handed(vector_register)
                    },
                    slot: 0
                }
            ),
        ]
    );
}

/// `declaration` as a mixin with what its bodies are compiled from: its
/// slots, factory, initializer expressions and declaration.
fn mixin_of(declaration: &ns_syntax::ast::ClassDeclaration) -> crate::mixin::Mixin {
    let argument_count = declaration
        .factory
        .as_ref()
        .map_or(0, crate::interpreter::instantiate::pattern_parameter_count);
    crate::mixin::Mixin {
        slots: declaration
            .instance_side
            .slots()
            .map(|slot| crate::mixin::MixinSlot::from_declaration(Rc::new(slot.clone())))
            .collect(),
        factory: Some(crate::mixin::Factory {
            selector: Symbol::intern("factory"),
            argument_count,
        }),
        has_body: true,
        initializer_expressions: declaration.instance_side.initializer_expressions.clone(),
        declaration: Some(Rc::new(declaration.clone())),
        ..crate::mixin::Mixin::empty(Symbol::intern(&declaration.name.name))
    }
}

#[test]
fn an_initializer_s_prologue_slots_and_lazy_getter_are_collected() {
    let source = indoc! {"
        class Outer = Object new (
        ) (
          class Mixer = Object new () ()
          class Other = Object new () ()
          class Applied x: a = Object new <: Mixer new <: Other new (
            |
              s = a foo.
            |
          ) (
            lazy t = 3 bar.
          )
          class Bare = (
            |
              u = 1.
            |
          ) ()
        ) : ()
    "};
    let module = ns_syntax::parse(source).expect("parse");
    let resolved = match crate::resolve(
        module,
        Rc::from(source),
        crate::SourceOrigin::File(PathBuf::from("t.ns")),
    ) {
        Ok(resolved) => resolved,
        Err(_) => panic!("does not resolve"),
    };
    let nested = |name: &str| {
        resolved
            .module
            .root_class
            .instance_side
            .nested_classes()
            .find(|declaration| declaration.name.name == name)
            .expect("a nested class")
    };
    let collector = MetadataCollector::new();

    let applied = nested("Applied");
    let mut mixin = mixin_of(applied);
    crate::compiler::compile_mixin_bodies(
        &mut mixin,
        &resolved.bindings,
        Lowering::Optimized,
        Some(&collector),
    );
    let full = mixin.initializer_code.as_ref().expect("a full initializer");
    let metadata = collector.take(full).expect("the initializer's metadata");
    assert_eq!(
        statement_texts(&metadata, source),
        vec![
            "Mixer new <: Other new",
            "Object new",
            "Mixer new",
            "Other new",
            "s = a foo.",
        ]
    );
    for (position, entry) in metadata.statements.iter().enumerate() {
        assert_eq!(entry.instructions, statement_range(full, position));
    }
    assert_eq!(
        metadata.variables.to_vec(),
        vec![parameter("a", 0, VariablePlace::Register(0))]
    );
    let clause = mixin
        .superclass_clause_code
        .as_ref()
        .expect("a superclass clause body");
    assert!(collector.take(clause).is_some());

    let lazy = applied
        .instance_side
        .slots()
        .find(|slot| slot.lazy)
        .expect("a lazy slot");
    let getter = crate::compiler::compile_lazy_slot_getter(
        lazy.initializer.as_ref().expect("an initializer"),
        1,
        lazy.span,
        &resolved.bindings,
        Lowering::Optimized,
        Some(&collector),
    );
    assert!(matches!(
        getter.instruction(0),
        Instruction::ReturnSlotIfFilled { .. }
    ));
    let getter_metadata = collector.take(&getter).expect("the getter's metadata");
    assert_eq!(
        statement_texts(&getter_metadata, source),
        vec!["lazy t = 3 bar."]
    );
    assert_eq!(getter_metadata.statements[0].instructions.start, 1);
    assert_eq!(
        getter_metadata.statements[0].instructions,
        statement_range(&getter, 0)
    );

    let mut bare = mixin_of(nested("Bare"));
    crate::compiler::compile_mixin_bodies(
        &mut bare,
        &resolved.bindings,
        Lowering::Optimized,
        Some(&collector),
    );
    let full = bare.initializer_code.as_ref().expect("a full initializer");
    let metadata = collector.take(full).expect("the initializer's metadata");
    assert_eq!(statement_texts(&metadata, source), vec!["Bare", "u = 1."]);
}

/// `source`'s first method compiled under `lowering` with a collector, and
/// the collector, for a test that walks the whole family of bodies.
fn collected_under(source: &str, lowering: Lowering) -> (Rc<CompiledCode>, MetadataCollector) {
    let collector = MetadataCollector::new();
    let code = Rc::new(compile(source, lowering, Some(&collector)));
    (code, collector)
}

#[test]
fn both_lowerings_name_the_same_variables_of_a_block() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m: p = ( | t u | ^[:b | t + b + u. u:: 1. [:c | c + b ] ] )
        ) : ()
    "};
    let (optimized_method, optimized_collector) = collected_under(source, Lowering::Optimized);
    let (reference_method, reference_collector) = collected_under(source, Lowering::Reference);
    let names = |metadata: &BodyMetadata| {
        metadata
            .variables
            .iter()
            .map(|entry| (entry.name, entry.depth))
            .collect::<Vec<_>>()
    };
    let optimized_method_metadata = optimized_collector
        .take(&optimized_method)
        .expect("the optimized method's metadata");
    let reference_method_metadata = reference_collector
        .take(&reference_method)
        .expect("the reference method's metadata");
    assert_eq!(
        names(&optimized_method_metadata),
        names(&reference_method_metadata)
    );
    // The reference lowering keeps every variable in a register of its
    // body, in the order the resolver numbers them: the parameter, then
    // the temporaries past the header's numbers.
    let temporary = |position: u16| HEADER_REGISTERS + position;
    assert_eq!(
        reference_method_metadata.variables.to_vec(),
        vec![
            parameter("p", 0, VariablePlace::Register(0)),
            variable("t", 0, VariablePlace::Register(temporary(1))),
            variable("u", 0, VariablePlace::Register(temporary(2))),
        ]
    );

    let optimized_block = optimized_collector
        .take(&first_block(&optimized_method))
        .expect("the optimized block's metadata");
    let reference_block = reference_collector
        .take(&first_block(&reference_method))
        .expect("the reference block's metadata");
    assert_eq!(names(&optimized_block), names(&reference_block));
    // Every enclosing variable is read through the captured frames, at the
    // register its declaring body keeps it in.
    let outer = |register: u16| VariablePlace::Outer { hops: 1, register };
    assert_eq!(
        reference_block.variables.to_vec(),
        vec![
            parameter("b", 0, VariablePlace::Register(0)),
            parameter("p", 1, outer(0)),
            variable("t", 1, outer(temporary(1))),
            variable("u", 1, outer(temporary(2))),
        ]
    );

    // A block two frames in reads its method's variables two hops out.
    let optimized_inner = optimized_collector
        .take(&first_block(&first_block(&optimized_method)))
        .expect("the optimized inner block's metadata");
    let reference_inner = reference_collector
        .take(&first_block(&first_block(&reference_method)))
        .expect("the reference inner block's metadata");
    assert_eq!(names(&optimized_inner), names(&reference_inner));
    let twice_out = |register: u16| VariablePlace::Outer { hops: 2, register };
    assert_eq!(
        reference_inner.variables.to_vec(),
        vec![
            parameter("c", 0, VariablePlace::Register(0)),
            parameter("b", 1, outer(0)),
            parameter("p", 2, twice_out(0)),
            variable("t", 2, twice_out(temporary(1))),
            variable("u", 2, twice_out(temporary(2))),
        ]
    );
    // No block reads `p`, so the optimized inner block reads it where
    // its method declares it, two hops out (decision 10).
    let parameter = optimized_inner
        .variables
        .iter()
        .find(|entry| entry.name == Symbol::intern("p"))
        .expect("p is named");
    assert_eq!(parameter.place, twice_out(0));
}

/// Every (receiver, selector) pair of the sends the family of bodies of
/// `code` records, as text, sorted.
fn send_pairs(
    code: &Rc<CompiledCode>,
    collector: &MetadataCollector,
    source: &str,
) -> Vec<(Option<String>, Vec<String>)> {
    let mut bodies = Vec::new();
    family(code, &mut bodies);
    let mut pairs = Vec::new();
    for body in &bodies {
        let metadata = collector.take(body).expect("every body's metadata");
        for entry in metadata.sends.iter() {
            let (receiver, selector) = send_texts(entry, source);
            pairs.push((
                receiver.map(str::to_owned),
                selector.into_iter().map(str::to_owned).collect(),
            ));
        }
    }
    pairs.sort();
    pairs
}

#[test]
fn both_lowerings_record_the_same_sends_by_span() {
    // No send sits inside an inlined block: the optimized lowering lowers
    // such a block twice, inline and as the fallback closure of the
    // conditional's fallback send, and records the sends of both, where
    // the reference lowering lowers it once. A loop is left out too, as
    // its own test says.
    let source = indoc! {"
        class X = Object new (
        | s ::= 0. |
        ) (
          m: x with: y = (
            foo.
            s:: 3.
            x foo; bar: 1 baz: 2; qux.
            y at: 1 put: x.
            x + 1.
            x ifTrue: [ 2 ] ifFalse: [ 3 ].
            x and: [ y ].
            ^[:b | b zip: x ]
          )
          foo = ( )
        ) : ()
    "};
    let (optimized_code, optimized) = collected_under(source, Lowering::Optimized);
    let (reference_code, reference) = collected_under(source, Lowering::Reference);
    let expected = send_pairs(&optimized_code, &optimized, source);
    assert_eq!(expected.len(), 10, "{expected:?}");
    assert_eq!(send_pairs(&reference_code, &reference, source), expected);
}

#[test]
fn a_while_loop_s_value_sends_name_its_source_send_in_the_reference_lowering() {
    // The optimized lowering emits no send for a `whileTrue:` loop, so the
    // two lowerings differ here by exactly the loop's two `value` sends,
    // which the reference lowering gives the loop's spans.
    let source = indoc! {"
        class X = Object new (
        ) (
          m: x = ( [ x foo ] whileTrue: [ x bar ]. ^x )
        ) : ()
    "};
    let (optimized_code, optimized) = collected_under(source, Lowering::Optimized);
    let (reference_code, reference) = collected_under(source, Lowering::Reference);
    let optimized_pairs = send_pairs(&optimized_code, &optimized, source);
    let mut reference_pairs = send_pairs(&reference_code, &reference, source);
    let loop_send = (Some("[ x foo ]".to_owned()), vec!["whileTrue:".to_owned()]);
    assert!(!optimized_pairs.contains(&loop_send));
    let before = reference_pairs.len();
    reference_pairs.retain(|pair| *pair != loop_send);
    assert_eq!(before - reference_pairs.len(), 2);
    assert_eq!(reference_pairs, optimized_pairs);
}

#[test]
fn a_reference_send_entry_is_uncached() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m: x = ( x ifTrue: [ 1 ]. [ x foo ] whileFalse: [ x ]. ^x bar: 2 )
        ) : ()
    "};
    let (code, collector) = collected_under(source, Lowering::Reference);
    let mut bodies = Vec::new();
    family(&code, &mut bodies);
    let mut entries = 0;
    for body in &bodies {
        let metadata = collector.take(body).expect("every body's metadata");
        let sends: Vec<u32> = (0..body.instruction_count())
            .filter(|index| body.instruction(*index).call_site().is_some())
            .map(|index| u32::try_from(index).expect("small"))
            .collect();
        assert_eq!(
            metadata
                .sends
                .iter()
                .map(|entry| entry.index)
                .collect::<Vec<_>>(),
            sends,
            "an entry for every send instruction and no other"
        );
        for entry in metadata.sends.iter() {
            assert_eq!(entry.call_site, Some(crate::bytecode::CallSite::UNCACHED));
            entries += 1;
        }
        for (position, entry) in metadata.statements.iter().enumerate() {
            assert_eq!(entry.instructions, statement_range(body, position));
        }
    }
    // `ifTrue:`, `bar:`, `whileFalse:`'s two `value` sends and the
    // condition block's `foo`.
    assert_eq!(entries, 5);
}

/// Every block literal a send of the family of bodies of `code` takes, by
/// its text, with whether the send is an inlined form, each once: the
/// optimized lowering lowers an inlined block twice, inline and as its
/// fallback closure, and records the sends of both.
fn taken_blocks(
    code: &Rc<CompiledCode>,
    collector: &MetadataCollector,
    source: &str,
) -> Vec<(String, bool)> {
    let mut bodies = Vec::new();
    family(code, &mut bodies);
    let mut taken = std::collections::BTreeSet::new();
    for body in &bodies {
        let metadata = collector.take(body).expect("every body's metadata");
        for entry in metadata.block_sends.iter() {
            for block in entry.blocks.iter() {
                taken.insert((block.span.text(source).to_owned(), entry.inlined));
            }
        }
    }
    taken.into_iter().collect()
}

#[test]
fn both_lowerings_record_the_same_taken_blocks() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m: x = (
            x ifTrue: [ x foo ].
            x and: [ x bar ].
            [ x baz ] whileTrue: [ x qux ].
            x do: [:each | each ifFalse: [ x zip ] ].
            [ x boom ] on: x do: [:error | error ].
            self with: [ 1 ].
            with: [ 3 ].
            ^[ 2 ]
          )
          with: block = ( ^block value )
        ) : ()
    "};
    let (optimized_code, optimized) = collected_under(source, Lowering::Optimized);
    let (reference_code, reference) = collected_under(source, Lowering::Reference);
    let expected: Vec<(String, bool)> = [
        ("[ 1 ]", false),
        ("[ 3 ]", false),
        ("[ x bar ]", true),
        ("[ x baz ]", true),
        ("[ x boom ]", false),
        ("[ x foo ]", true),
        ("[ x qux ]", true),
        ("[ x zip ]", true),
        ("[:each | each ifFalse: [ x zip ] ]", false),
        ("[:error | error ]", false),
    ]
    .into_iter()
    .map(|(text, inlined)| (text.to_owned(), inlined))
    .collect();
    assert_eq!(taken_blocks(&optimized_code, &optimized, source), expected);
    assert_eq!(taken_blocks(&reference_code, &reference, source), expected);
}

#[test]
fn an_inlined_loop_records_its_blocks_inline_and_where_it_goes_on() {
    let source = indoc! {"
        class X = Object new (
        ) (
          m: x = ( [ x foo ] whileTrue: [ x bar ]. ^x )
        ) : ()
    "};
    let (code, metadata, _) = collected(source);
    let [entry] = &metadata.block_sends[..] else {
        panic!("one block send: {:?}", metadata.block_sends);
    };
    assert!(entry.inlined);
    let start_of = |text: &str| {
        metadata
            .statements
            .iter()
            .find(|statement| statement.span.text(source) == text)
            .map(|statement| statement.instructions.start)
            .unwrap_or_else(|| panic!("the statement {text}"))
    };
    // Each block's statements lie in its own instructions, and the loop
    // goes on at the next statement once it ends.
    for (block, statement) in entry.blocks.iter().zip(["x foo", "x bar"]) {
        let super::BlockPlace::Inline(instructions) = &block.place else {
            panic!("an inline block: {block:?}");
        };
        assert!(instructions.contains(&start_of(statement)), "{block:?}");
    }
    assert_eq!(entry.after, start_of("^x"));
    assert!(usize::try_from(entry.after).is_ok_and(|after| after < code.instruction_count()));
}

#[test]
fn both_lowerings_record_the_same_initializer_statements() {
    let source = indoc! {"
        class Outer = Object new (
        ) (
          class Mixer = Object new () ()
          class Applied x: a = Object new <: Mixer new (
            |
              s = a foo.
              u = 1.
            |
          ) (
            lazy t = 3 bar.
          )
          class Bare = (
            |
              v = 1.
            |
          ) ()
        ) : ()
    "};
    let module = ns_syntax::parse(source).expect("parse");
    let Ok(resolved) = crate::resolve(
        module,
        Rc::from(source),
        crate::SourceOrigin::File(PathBuf::from("t.ns")),
    ) else {
        panic!("does not resolve");
    };
    let statements_of = |name: &str, lowering: Lowering| {
        let declaration = resolved
            .module
            .root_class
            .instance_side
            .nested_classes()
            .find(|declaration| declaration.name.name == name)
            .expect("a nested class");
        let collector = MetadataCollector::new();
        let mut mixin = mixin_of(declaration);
        crate::compiler::compile_mixin_bodies(
            &mut mixin,
            &resolved.bindings,
            lowering,
            Some(&collector),
        );
        let full = mixin.initializer_code.as_ref().expect("a full initializer");
        let metadata = collector.take(full).expect("the initializer's metadata");
        for (position, entry) in metadata.statements.iter().enumerate() {
            assert_eq!(entry.instructions, statement_range(full, position));
        }
        let clause = mixin.superclass_clause_code.as_ref().map(|clause| {
            collector
                .take(clause)
                .expect("the superclass clause's metadata")
        });
        (
            metadata
                .statements
                .iter()
                .map(|entry| entry.span)
                .collect::<Vec<_>>(),
            metadata.variables.to_vec(),
            clause.map(|clause| -> Vec<_> {
                clause.statements.iter().map(|entry| entry.span).collect()
            }),
        )
    };
    for name in ["Applied", "Bare"] {
        assert_eq!(
            statements_of(name, Lowering::Reference),
            statements_of(name, Lowering::Optimized),
            "{name}"
        );
    }
    let (spans, _, _) = statements_of("Bare", Lowering::Reference);
    let texts: Vec<_> = spans.iter().map(|span| span.text(source)).collect();
    assert_eq!(texts, vec!["Bare", "v = 1."]);
}

#[test]
fn compiling_the_reference_lowering_without_a_collector_emits_the_same_code() {
    let source = indoc! {"
        class X = Object new (
        | s ::= 0. |
        ) (
          m: p = (
            | t u a = 1. |
            u:: p + 1.
            s:: t.
            p foo; bar: [ a ].
            [ u < 3 ] whileTrue: [ u:: u + 1 ].
            ^u
          )
        ) : ()
    "};
    let collector = MetadataCollector::new();
    assert_eq!(
        compile(source, Lowering::Reference, None),
        compile(source, Lowering::Reference, Some(&collector))
    );
}

// Compiling a declaration's level again

/// `source` resolved as the file `t.ns`.
fn resolved(source: &str) -> crate::ResolvedModule {
    let module = ns_syntax::parse(source).expect("parse");
    match crate::resolve(
        module,
        Rc::from(source),
        crate::SourceOrigin::File(PathBuf::from("t.ns")),
    ) {
        Ok(resolved) => resolved,
        Err(_) => panic!("does not resolve"),
    }
}

/// An interpreter of `lowering` with `source` loaded, and its root class.
fn loaded_under(source: &str, lowering: Lowering) -> (Interpreter, Root) {
    let mut interpreter = Interpreter::bootstrap_with(lowering);
    let class = interpreter.load_module(&resolved(source));
    (interpreter, class)
}

/// The mixin the class object `class` applies.
fn applied_mixin(mutator: &Mutator<'_>, class: &Root) -> Rc<Mixin> {
    let class = mutator.fetch(class).as_object().expect("a class object");
    Rc::clone(
        &mutator
            .heap()
            .class_application(class)
            .expect("a class object")
            .mixin,
    )
}

/// The declaration `name` nested in `mixin`, as the heap holds it.
fn nested_mixin(mutator: &Mutator<'_>, mixin: &Rc<Mixin>, name: &str) -> Rc<Mixin> {
    mutator
        .heap()
        .nested_mixins(mixin)
        .into_iter()
        .find(|nested| nested.name.as_str() == name)
        .expect("the declaration is nested there")
}

/// Every body `mixin` carries, blocks included, by address.
fn bodies_of(mixin: &Mixin) -> HashSet<usize> {
    let mut bodies = HashSet::new();
    crate::image::version_bodies(mixin, &mut bodies);
    bodies
}

/// The paired bodies' addresses, each with whether it got tables.
fn paired(pairs: &PairedBodies) -> HashMap<usize, bool> {
    pairs
        .iter()
        .map(|(code, metadata)| (Rc::as_ptr(code) as usize, metadata.is_some()))
        .collect()
}

/// The factory entry's body of the class side `class_side`.
fn factory_code(class_side: &Mixin) -> Rc<CompiledCode> {
    class_side
        .methods
        .values()
        .find_map(|entry| match entry {
            crate::mixin::MethodEntry::Factory(factory) => Some(Rc::clone(&factory.code)),
            _ => None,
        })
        .expect("a factory entry")
}

/// The body of the method `selector` of `mixin`.
fn method_code(mixin: &Mixin, selector: &str) -> Rc<CompiledCode> {
    Rc::clone(
        &mixin
            .compiled_method(Symbol::intern(selector))
            .expect("the method")
            .code,
    )
}

fn method_origin(mixin: &Rc<Mixin>, selector: &str) -> BodyOrigin {
    BodyOrigin {
        mixin: Rc::clone(mixin),
        member: Member::Method(Symbol::intern(selector)),
    }
}

crate::for_each_lowering! {
    fn every_body_of_a_level_compiles_again_alike(lowering) {
        let source = indoc! {"
            class Outer = Object new (
            ) (
              class Mixer = Object new () ()
              public class Applied x: a y: b = Object new <: Mixer new (
                |
                  s = a foo.
                  t ::= b.
                  u v = 3.
                |
                s bar.
              ) (
                lazy w = 3 bar.
                public m: p = ( | q | q:: p. ^[:z | q + z + s ] )
              ) : (
                public make = ( ^self x: 1 y: [ 2 ] )
              )
            ) : ()
        "};
        let (mut interpreter, outer) = loaded_under(source, lowering);
        interpreter.enter(|mutator| {
            let outer_mixin = applied_mixin(mutator, &outer);
            let applied = nested_mixin(mutator, &outer_mixin, "Applied");
            let class_side = applied.class_side.clone().expect("a declared class side");
            let mut expected = bodies_of(&applied);
            expected.extend(bodies_of(&class_side));
            expected.insert(Rc::as_ptr(&factory_code(&class_side)) as usize);
            assert!(expected.len() >= 9, "{} bodies", expected.len());

            let pairs = compile_level_again(mutator, &applied, lowering).expect("the level");
            let paired_bodies = paired(&pairs);
            for body in &expected {
                assert_eq!(paired_bodies.get(body), Some(&true), "every body pairs with tables");
            }
            assert_eq!(paired_bodies.len(), expected.len(), "no body of another level");

            // A class side is compiled again from its instance side's
            // declaration, and pairs alone.
            let pairs = compile_level_again(mutator, &class_side, lowering).expect("the level");
            let paired_bodies = paired(&pairs);
            let class_side_bodies = bodies_of(&class_side);
            assert!(class_side_bodies
                .iter()
                .all(|body| paired_bodies.get(body) == Some(&true)));
            assert_eq!(paired_bodies.len(), class_side_bodies.len() + 1, "and the factory");

            // The factory takes the header's parameters and its pattern.
            let factory = factory_code(&class_side);
            let origin = BodyOrigin {
                mixin: Rc::clone(&class_side),
                member: Member::Factory,
            };
            let metadata = Cache::metadata(mutator, &factory, &origin).expect("the factory's");
            assert_eq!(
                metadata.variables.to_vec(),
                vec![
                    parameter("a", 0, VariablePlace::Register(0)),
                    parameter("b", 0, VariablePlace::Register(1)),
                ]
            );
            assert!(metadata.statements.is_empty());
            assert!(metadata.sends.is_empty());
            let pattern = metadata.factory_pattern.expect("where it stands");
            assert_eq!(pattern.text(source), "x: a y: b");

            // A factory body a host's `instantiate` builds, which no mixin
            // holds, takes the header's too.
            let built = Rc::new(crate::compiler::factory_body(2));
            assert_eq!(Cache::metadata(mutator, &built, &origin), Some(metadata));
        });
    }
}

crate::for_each_lowering! {
    /// Every level of a module compiled again keeps the one text of the
    /// module the cache holds for its root, not a copy each; an install
    /// clears it with the metadata.
    fn the_levels_of_a_module_share_its_text(lowering) {
        let source = indoc! {"
            class Outer = Object new () (
              class Inner = Object new () (
                public inner = ( ^1 )
              )
              public top = ( ^2 )
            ) : ()
        "};
        let (mut interpreter, outer) = loaded_under(source, lowering);
        interpreter.enter(|mutator| {
            let outer_mixin = applied_mixin(mutator, &outer);
            let inner_mixin = nested_mixin(mutator, &outer_mixin, "Inner");
            let texts: Vec<Rc<str>> = [(&outer_mixin, "top"), (&inner_mixin, "inner")]
                .into_iter()
                .map(|(mixin, selector)| {
                    let code = method_code(mixin, selector);
                    let (_, text) =
                        Cache::metadata_with_text(mutator, &code, &method_origin(mixin, selector))
                            .expect("the method's metadata");
                    text
                })
                .collect();
            assert_eq!(&*texts[0], source);
            assert!(Rc::ptr_eq(&texts[0], &texts[1]), "one text for both levels");
            mutator.debug_metadata.clear();
            let code = method_code(&inner_mixin, "inner");
            let (_, again) =
                Cache::metadata_with_text(mutator, &code, &method_origin(&inner_mixin, "inner"))
                    .expect("the method's metadata");
            assert!(!Rc::ptr_eq(&texts[0], &again), "compiled afresh after a clear");
        });
    }
}

crate::for_each_lowering! {
    /// The families of one level placed in one flush share the level's
    /// compile from the module's current text, which the flush's end lets
    /// go of, since its pairs hold every running body of the level.
    fn the_placements_of_one_level_share_its_compile(lowering) {
        let source = indoc! {"
            class Outer = Object new () (
              public top = ( ^[:each | each + 1] )
              public other = ( ^2 )
            ) : ()
        "};
        let (mut interpreter, outer) = loaded_under(source, lowering);
        interpreter.enter(|mutator| {
            let outer_mixin = applied_mixin(mutator, &outer);
            for selector in ["top", "other"] {
                let code = method_code(&outer_mixin, selector);
                let placement =
                    Cache::own_placement(mutator, &code, &method_origin(&outer_mixin, selector))
                        .expect("the method placed");
                assert!(placement.metadata.is_some(), "{selector} has its metadata");
                assert!(placement.current.is_some(), "{selector} stands in the current text");
            }
            assert_eq!(mutator.debug_metadata.shared_levels(), 1, "one compile for both");
            mutator.debug_metadata.end_placements();
            assert_eq!(mutator.debug_metadata.shared_levels(), 0);
        });
    }
}

#[test]
fn a_body_with_an_unwind_marker_compiles_again_alike() {
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let mut interpreter = Interpreter::bootstrap_with(lowering);
        interpreter.enter(|mutator| {
            let closure = mutator.builtin_class(crate::BuiltinClass::Closure);
            let mixin = Rc::clone(
                &mutator
                    .heap()
                    .class_application(closure)
                    .expect("a class object")
                    .mixin,
            );
            let ensure = method_code(&mixin, "ensure:");
            assert!(ensure.unwind_marker.is_some());
            let pairs = compile_level_again(mutator, &mixin, lowering).expect("the level");
            let paired_bodies = paired(&pairs);
            assert_eq!(
                paired_bodies.get(&(Rc::as_ptr(&ensure) as usize)),
                Some(&true)
            );
            for body in bodies_of(&mixin) {
                assert_eq!(
                    paired_bodies.get(&body),
                    Some(&true),
                    "every body of Closure pairs"
                );
            }
        });
    }
}

#[test]
fn compiles_alike_ignores_spans_but_not_instructions() {
    let here = compile(
        indoc! {"
            class X = Object new (
            ) (
              m: p = ( ^[:b | p + b ] )
            ) : ()
        "},
        Lowering::Optimized,
        None,
    );
    let moved = compile(
        indoc! {"
            class X = Object new (
            ) (
              /// A comment that moves the method.
              m: p = ( ^[:b | p + b ] )
            ) : ()
        "},
        Lowering::Optimized,
        None,
    );
    let other = compile(
        indoc! {"
            class X = Object new (
            ) (
              m: p = ( ^[:b | p - b ] )
            ) : ()
        "},
        Lowering::Optimized,
        None,
    );
    assert_ne!(here.source, moved.source);
    assert_ne!(here, moved, "`==` compares the spans, the block's too");
    assert!(compiles_alike(&here, &moved));
    assert!(compiles_alike(&moved, &here));
    assert!(
        !compiles_alike(&here, &other),
        "the block's instructions differ"
    );
    let mut marked = here.clone();
    marked.unwind_marker = Some(crate::bytecode::UnwindMarker::Ensure {
        block_register: 0,
        complete_register: 1,
    });
    assert!(compiles_alike(&marked, &here));
    let mut counted = here.clone();
    counted.register_count += 1;
    assert!(!compiles_alike(&counted, &here));
}

#[test]
fn an_install_moves_the_spans_of_an_unedited_body() {
    let source = indoc! {"
        class Root = Object new (
        ) (
          public class A = () (
            public a = ( ^1 )
          )
          public class B = () (
            public m = ( ^2 )
          )
        ) : ()
    "};
    let (mut interpreter, root) = loaded_under(source, Lowering::Optimized);
    let (before, after) = interpreter.enter(|mutator| {
        let root_mixin = applied_mixin(mutator, &root);
        let first = nested_mixin(mutator, &root_mixin, "A");
        let second = nested_mixin(mutator, &root_mixin, "B");
        let code = method_code(&second, "m");
        let origin = method_origin(&second, "m");
        let before = Cache::metadata(mutator, &code, &origin).expect("B>>m's metadata");
        let first_id = mutator.heap().known_mixin_id(&first).expect("A's id");
        let edit = crate::interpreter::edit::Edit {
            header: None,
            instance_side: vec![crate::interpreter::edit::MemberEdit::AddMethod(
                "public longer = ( ^3 + 4 )".to_string(),
            )],
            class_side: Vec::new(),
            new_name: None,
        };
        if let Err(diagnostic) = mutator.install_one(first_id, &edit) {
            panic!("the install was refused: {diagnostic}");
        }
        let root_id = mutator
            .heap()
            .known_mixin_id(&root_mixin)
            .expect("Root's id");
        let text = mutator
            .heap()
            .module_source(root_id)
            .expect("the module's text");
        let after = Cache::metadata(mutator, &code, &origin).expect("B>>m's metadata");
        assert_eq!(after.statements[0].span.text(&text), "^2");
        (before, after)
    });
    assert_eq!(before.statements[0].span.text(source), "^2");
    assert!(
        after.statements[0].span.start > before.statements[0].span.start,
        "the statement stands where the lengthened text now has it"
    );
}

/// A module whose nested class `Inner` holds a method with a block.
const WITH_INNER: &str = indoc! {"
    class Root = Object new (
    ) (
      public class Inner = () (
        public m: x = ( ^[:y | x foo: y ] )
      )
    ) : ()
"};

/// `source`'s module written with its source by an interpreter of
/// `lowering`.
fn image_under(source: &str, lowering: Lowering) -> Vec<u8> {
    let (mut interpreter, class) = loaded_under(source, lowering);
    interpreter.enter(|mutator| {
        let class = mutator.fetch(&class).as_object().expect("a class object");
        let name = mutator.heap.class_declaration(class).expect("a root").name;
        let identity = crate::modules::ModuleIdentity::undeclared("t.ns", name.as_str());
        let modules = [crate::image::ModuleSource {
            class,
            identity: &identity,
            with_source: true,
        }];
        crate::image::snapshot(mutator, &modules, &[]).expect("snapshot")
    })
}

fn loaded_image(bytes: &[u8]) -> crate::image::LoadedImage {
    match crate::image::load(bytes) {
        Ok(image) => image,
        Err(message) => panic!("load: {message}"),
    }
}

/// `bytes` with every `from` replaced by `into`, of the same length.
fn patched(bytes: &[u8], from: &str, into: &str) -> Vec<u8> {
    assert_eq!(from.len(), into.len());
    let mut bytes = bytes.to_vec();
    let mut found = 0;
    let mut index = 0;
    while index + from.len() <= bytes.len() {
        if &bytes[index..index + from.len()] == from.as_bytes() {
            bytes[index..index + from.len()].copy_from_slice(into.as_bytes());
            found += 1;
        }
        index += 1;
    }
    assert!(found > 0, "the image holds {from}");
    bytes
}

#[test]
fn a_crafted_text_without_the_declaration_gives_no_metadata() {
    let bytes = image_under(WITH_INNER, Lowering::Optimized);
    // The text names another class, and then does not parse; the
    // symbols, which name the mixins, are untouched.
    for (from, into) in [("class Inner", "class Innex"), ("^[:y", "^[:)")] {
        let mut image = loaded_image(&patched(&bytes, from, into));
        let root = Rc::clone(&image.modules[0].declaration);
        image.interpreter.enter(|mutator| {
            let inner = nested_mixin(mutator, &root, "Inner");
            assert!(compile_level_again(mutator, &inner, Lowering::Optimized).is_none());
            let code = method_code(&inner, "m:");
            let origin = method_origin(&inner, "m:");
            assert!(Cache::metadata(mutator, &code, &origin).is_none());
            // Cached as none.
            assert!(Cache::metadata(mutator, &code, &origin).is_none());
        });
    }
}

#[test]
fn a_reference_lowered_image_body_pairs_with_the_reference_lowering() {
    let bytes = image_under(WITH_INNER, Lowering::Reference);
    let mut image = loaded_image(&bytes);
    let root = Rc::clone(&image.modules[0].declaration);
    image.interpreter.enter(|mutator| {
        assert!(mutator.lowering == Lowering::Optimized);
        let inner = nested_mixin(mutator, &root, "Inner");
        let code = method_code(&inner, "m:");
        let block = first_block(&code);
        // The optimized lowering compiles the method otherwise.
        let optimized =
            compile_level_again(mutator, &inner, Lowering::Optimized).expect("the level");
        assert_eq!(
            paired(&optimized).get(&(Rc::as_ptr(&code) as usize)),
            Some(&false)
        );
        let origin = method_origin(&inner, "m:");
        let metadata = Cache::metadata(mutator, &code, &origin).expect("the method's metadata");
        assert!(metadata.sends.is_empty());
        let block_metadata = Cache::metadata(mutator, &block, &origin).expect("the block's");
        assert_eq!(block_metadata.sends.len(), 1);
        assert_eq!(
            block_metadata.sends[0].call_site,
            Some(crate::bytecode::CallSite::UNCACHED)
        );
    });
}

// spec: vm.compiler.debug-tables-describe-their-body
#[test]
fn a_body_that_compiles_again_otherwise_fails_the_check() {
    let source = indoc! {"
        class Root = Object new (
        ) (
          public class Inner = () (
            public m = ( ^11 )
          )
        ) : ()
    "};
    let bytes = image_under(source, Lowering::Optimized);
    match loaded_image(&bytes).interpreter.check_debug_metadata(false) {
        Ok(count) => assert!(count > 0),
        Err(failure) => panic!("{failure}"),
    }
    // The text answers another literal than the code the image carries.
    let mut image = loaded_image(&patched(&bytes, "( ^11 )", "( ^12 )"));
    match image.interpreter.check_debug_metadata(false) {
        Ok(count) => panic!("{count} bodies passed"),
        Err(failure) => assert!(
            failure.contains("Inner>>m does not compile again alike"),
            "{failure}"
        ),
    }
}

/// A module whose nested class `Point` has a factory and no class side of
/// its own, below a class an edit lengthens.
const A_FACTORY_BELOW: &str = indoc! {"
    class Root = Object new (
    ) (
      public class A = () (
        public a = ( ^1 )
      )
      public class Point x: ax = (
        |
          public x = ax.
        |
      ) ()
    ) : ()
"};

/// The description of the factory body of `class_side`, a version of
/// `Point`'s class side.
fn factory_description(
    mutator: &mut Mutator<'_>,
    class_side: &Rc<Mixin>,
) -> crate::trace_events::descriptions::BodyDescription {
    let origin = BodyOrigin {
        mixin: Rc::clone(class_side),
        member: Member::Factory,
    };
    let body = crate::trace_events::BodyName {
        number: 1,
        name: "Root/Point class>>x:".to_string(),
    };
    crate::trace_events::descriptions::describe(
        mutator,
        &factory_code(class_side),
        &origin,
        body,
        None,
    )
    .expect("the factory is described")
}

/// `Point`'s class side as the heap holds it now: synthesized, since the
/// declaration declares none.
fn point_class_side(mutator: &mut Mutator<'_>, root: &Root) -> Rc<Mixin> {
    let root_mixin = applied_mixin(mutator, root);
    let point = nested_mixin(mutator, &root_mixin, "Point");
    mutator.metaclass_mixin(&point)
}

/// A synthesized body is placed where its own version declared it: after
/// an edit above it, at its header moved in the current text and at its
/// own in the text it was read in; after an edit that renamed its factory
/// and kept its arity, in its own text alone, never at the new header,
/// which its old version compiles alike with.
#[test]
fn a_synthesized_body_is_placed_where_its_version_declared_it() {
    let source = A_FACTORY_BELOW;
    let (mut interpreter, root) = loaded_under(source, Lowering::Optimized);
    interpreter.enter(|mutator| {
        let slice = |span: ns_syntax::Span| &source[span.start as usize..span.end as usize];
        let root_mixin = applied_mixin(mutator, &root);
        let first = nested_mixin(mutator, &root_mixin, "A");
        let first_id = mutator.heap().known_mixin_id(&first).expect("A's id");
        let lengthening = crate::interpreter::edit::Edit {
            header: None,
            instance_side: vec![crate::interpreter::edit::MemberEdit::AddMethod(
                "public longer = ( ^3 + 4 )".to_string(),
            )],
            class_side: Vec::new(),
            new_name: None,
        };
        if let Err(diagnostic) = mutator.install_one(first_id, &lengthening) {
            panic!("the install was refused: {diagnostic}");
        }
        mutator.debug_metadata.clear();
        let original = point_class_side(mutator, &root);
        let moved = factory_description(mutator, &original);
        assert_eq!(slice(moved.source), "x: ax");
        let current = moved.current.as_ref().expect("the current text holds it");
        assert_eq!(
            &current.text[current.source.start as usize..current.source.end as usize],
            "x: ax"
        );
        assert!(
            current.source.start > moved.source.start,
            "the edit moved it"
        );
        assert_eq!(
            moved.checksum,
            crate::trace_events::descriptions::source_checksum(b"x: ax")
        );

        let root_mixin = applied_mixin(mutator, &root);
        let point = nested_mixin(mutator, &root_mixin, "Point");
        let point_id = mutator.heap().known_mixin_id(&point).expect("Point's id");
        let renaming = crate::interpreter::edit::Edit {
            header: Some(
                "public class Point at: ax = (\n    |\n      public x = ax.\n    |\n  ) "
                    .to_string(),
            ),
            instance_side: Vec::new(),
            class_side: Vec::new(),
            new_name: None,
        };
        if let Err(diagnostic) = mutator.install_one(point_id, &renaming) {
            panic!("the install was refused: {diagnostic}");
        }
        mutator.debug_metadata.clear();
        let superseded = factory_description(mutator, &original);
        assert_eq!(slice(superseded.source), "x: ax");
        assert_eq!(superseded.checksum, moved.checksum);
        assert!(
            superseded.current.is_none(),
            "the current text's header is another's: {superseded:?}"
        );
        let renamed = point_class_side(mutator, &root);
        let edited = factory_description(mutator, &renamed);
        let current = edited.current.as_ref().expect("the current text holds it");
        assert_eq!(
            &current.text[current.source.start as usize..current.source.end as usize],
            "at: ax"
        );
    });
}
