use std::path::PathBuf;
use std::rc::Rc;

use indoc::indoc;

use super::{
    BodyMetadata, MetadataCollector, SendEntry, VariableEntry, VariablePlace, VectorHolder,
};
use crate::bytecode::{CompiledCode, Instruction, HEADER_REGISTERS};
use crate::compiler::Lowering;
use crate::Symbol;

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
