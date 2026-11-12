use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use indoc::indoc;

use super::again::{compile_level_again, BodyOrigin, Member, PairedBodies};
use super::{
    BodyMetadata, Cache, MetadataCollector, SendEntry, VariableEntry, VariablePlace, VectorHolder,
};
use crate::bytecode::{compiles_alike, CompiledCode, Instruction, HEADER_REGISTERS};
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
        depth,
        place,
    }
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
    // the vector register after them.
    let t_register = 1 + HEADER_REGISTERS;
    let vector_register = t_register + 2;
    assert_eq!(
        metadata.variables.to_vec(),
        vec![
            variable("p", 0, VariablePlace::Register(0)),
            variable("t", 0, VariablePlace::Register(t_register)),
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
            variable("b", 0, VariablePlace::Register(0)),
            variable(
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
            variable("t", 0, VariablePlace::Register(0)),
            variable(
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
            variable(
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
        vec![variable("a", 0, VariablePlace::Register(0))]
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
          m: p = ( | t u | ^[:b | t + b + u. u:: 1 ] )
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
            variable("p", 0, VariablePlace::Register(0)),
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
            variable("b", 0, VariablePlace::Register(0)),
            variable("p", 1, outer(0)),
            variable("t", 1, outer(temporary(1))),
            variable("u", 1, outer(temporary(2))),
        ]
    );
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
                    variable("a", 0, VariablePlace::Register(0)),
                    variable("b", 0, VariablePlace::Register(1)),
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
        };
        if let Err(diagnostic) = mutator.install_edit(first_id, &edit) {
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
        let origin = crate::SourceOrigin::File(PathBuf::from("t.ns"));
        let modules = [crate::image::ModuleSource {
            class,
            origin: &origin,
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
