//! The specification's table of register effects
//! (`docs/spec/bytecode.md`, `bytecode.register-effects`) held to the
//! exhaustive matches that decide them: `registers_read`,
//! `register_written`, `jump_target`, `falls_through`, `outer_reach_of` and
//! `addresses_own_slots_of`.

use std::collections::BTreeMap;
use std::rc::Rc;

use ns_syntax::Span;

use super::*;
use crate::builtins::primitive::MirrorPrimitive;
use crate::spec_tables::{INSTRUCTION_KINDS, bytecode_table, instruction_name};

/// One instruction of every kind, under the name its row has, with the
/// value of each of its register and target fields: every value distinct,
/// so that a column naming the wrong field fails.
struct Sample {
    name: &'static str,
    instruction: Instruction,
    fields: Vec<(&'static str, Vec<u32>)>,
}

fn sample(
    name: &'static str,
    instruction: Instruction,
    fields: &[(&'static str, &[u32])],
) -> Sample {
    Sample {
        name,
        instruction,
        fields: fields
            .iter()
            .map(|(field, values)| (*field, values.to_vec()))
            .collect(),
    }
}

/// A block whose deepest by-reference access is two frames out, so that a
/// `MakeClosure` of it reaches one.
fn block_reaching_two_frames_out() -> Rc<CompiledCode> {
    let code = CompiledCode::new(
        CodeKind::Block,
        vec![
            Instruction::LoadOuterLocal {
                destination: 14,
                depth: 2,
                index: 0,
            },
            Instruction::ImplicitReturn { register: 14 },
        ],
        0,
        15,
        0,
        vec![(0, 0..2)],
        Span::new(0, 1),
    );
    assert_eq!(code.outer_reach, 2);
    Rc::new(code)
}

// The samples are one long list, one per instruction, read top to bottom.
#[expect(
    clippy::too_many_lines,
    reason = "one sample per instruction, each a few lines"
)]
fn every_sample() -> Vec<Sample> {
    let selector = Symbol::intern("with:with:");
    let site = CallSite { number: 0 };
    vec![
        sample(
            "LoadLiteral",
            Instruction::LoadLiteral {
                destination: 1,
                literal: Literal::Nil,
            },
            &[("destination", &[1])],
        ),
        sample(
            "LoadLiteral/BodyTooLarge",
            Instruction::LoadLiteral {
                destination: 1,
                literal: Literal::BodyTooLarge(BodyLimit::Registers),
            },
            &[("destination", &[1])],
        ),
        sample(
            "LoadSelf",
            Instruction::LoadSelf { destination: 1 },
            &[("destination", &[1])],
        ),
        sample(
            "LoadEnclosingObject",
            Instruction::LoadEnclosingObject {
                destination: 1,
                depth: 2,
            },
            &[("destination", &[1])],
        ),
        sample(
            "Move",
            Instruction::Move {
                destination: 1,
                source: 2,
            },
            &[("destination", &[1]), ("source", &[2])],
        ),
        sample(
            "StoreLocal",
            Instruction::StoreLocal { source: 2, slot: 3 },
            &[("source", &[2]), ("slot", &[3])],
        ),
        sample(
            "LoadOuterLocal",
            Instruction::LoadOuterLocal {
                destination: 1,
                depth: 3,
                index: 4,
            },
            &[("destination", &[1])],
        ),
        sample(
            "StoreOuterLocal",
            Instruction::StoreOuterLocal {
                source: 2,
                depth: 3,
                index: 4,
            },
            &[("source", &[2])],
        ),
        sample(
            "SendLexical",
            Instruction::SendLexical {
                depth: 1,
                selector,
                argument_base: 20,
                argument_sources: vec![4, 5],
                destination: 6,
                call_site: site,
            },
            &[("argument_sources", &[4, 5]), ("destination", &[6])],
        ),
        sample(
            "SendPublic",
            Instruction::SendPublic {
                selector,
                receiver_register: 3,
                argument_base: 20,
                argument_sources: vec![4, 5],
                destination: 6,
                call_site: site,
            },
            &[
                ("receiver_register", &[3]),
                ("argument_sources", &[4, 5]),
                ("destination", &[6]),
            ],
        ),
        sample(
            "SendInlinable",
            Instruction::SendInlinable {
                selector,
                receiver_register: 3,
                argument_base: 20,
                argument_sources: vec![4, 5],
                destination: 6,
                call_site: site,
            },
            &[
                ("receiver_register", &[3]),
                ("argument_sources", &[4, 5]),
                ("destination", &[6]),
            ],
        ),
        sample(
            "SendSuper",
            Instruction::SendSuper {
                selector,
                argument_base: 20,
                argument_sources: vec![4, 5],
                destination: 6,
                call_site: site,
            },
            &[("argument_sources", &[4, 5]), ("destination", &[6])],
        ),
        sample(
            "SendMirror",
            Instruction::SendMirror {
                primitive: MirrorPrimitive::from_index(0),
                mirror_register: 7,
                subject_register: 8,
                argument_base: 20,
                argument_sources: vec![4, 5],
                destination: 6,
                failure: 9,
            },
            &[
                ("mirror_register", &[7]),
                ("subject_register", &[8]),
                ("argument_sources", &[4, 5]),
                ("destination", &[6]),
                ("failure", &[9]),
            ],
        ),
        sample(
            "MakeClosure",
            Instruction::MakeClosure {
                destination: 1,
                parameters: Rc::new(Vec::new()),
                code: block_reaching_two_frames_out(),
                copied: vec![4, 5],
            },
            &[("destination", &[1]), ("copied", &[4, 5])],
        ),
        sample(
            "MakeArray",
            Instruction::MakeArray {
                destination: 1,
                element_registers: vec![4, 5, 7],
            },
            &[("destination", &[1]), ("element_registers", &[4, 5, 7])],
        ),
        sample(
            "MakeVector",
            Instruction::MakeVector {
                destination: 1,
                size: 3,
            },
            &[("destination", &[1])],
        ),
        sample(
            "LoadVector",
            Instruction::LoadVector {
                destination: 1,
                vector: 2,
                index: 3,
            },
            &[("destination", &[1]), ("vector", &[2])],
        ),
        sample(
            "StoreVector",
            Instruction::StoreVector {
                vector: 2,
                index: 3,
                source: 4,
            },
            &[("vector", &[2]), ("source", &[4])],
        ),
        sample(
            "InitializeSlot",
            Instruction::InitializeSlot {
                source: 2,
                own_index: 3,
            },
            &[("source", &[2])],
        ),
        sample(
            "ReturnSlotIfFilled",
            Instruction::ReturnSlotIfFilled { own_index: 3 },
            &[],
        ),
        sample(
            "MakeFuture",
            Instruction::MakeFuture {
                destination: 1,
                closure: 2,
            },
            &[("destination", &[1]), ("closure", &[2])],
        ),
        sample(
            "ResolveFuture",
            Instruction::ResolveFuture {
                destination: 1,
                source: 2,
            },
            &[("destination", &[1]), ("source", &[2])],
        ),
        sample(
            "LoadOwnSlot",
            Instruction::LoadOwnSlot {
                destination: 1,
                own_index: 3,
            },
            &[("destination", &[1])],
        ),
        sample(
            "InitializeSlotProvisionally",
            Instruction::InitializeSlotProvisionally {
                source: 2,
                own_index: 3,
            },
            &[("source", &[2])],
        ),
        sample(
            "Return",
            Instruction::Return { register: 2 },
            &[("register", &[2])],
        ),
        sample(
            "ImplicitReturn",
            Instruction::ImplicitReturn { register: 2 },
            &[("register", &[2])],
        ),
        sample(
            "NonLocalReturn",
            Instruction::NonLocalReturn { register: 2 },
            &[("register", &[2])],
        ),
        sample(
            "SignalNonBoolean",
            Instruction::SignalNonBoolean { register: 2 },
            &[("register", &[2])],
        ),
        sample("Jump", Instruction::Jump { target: 9 }, &[("target", &[9])]),
        sample(
            "JumpIfNotBoolean",
            Instruction::JumpIfNotBoolean {
                register: 2,
                target: 9,
            },
            &[("register", &[2]), ("target", &[9])],
        ),
        sample(
            "BranchIfFalse",
            Instruction::BranchIfFalse {
                register: 2,
                target: 9,
            },
            &[("register", &[2]), ("target", &[9])],
        ),
        sample(
            "BranchIfTrue",
            Instruction::BranchIfTrue {
                register: 2,
                target: 9,
            },
            &[("register", &[2]), ("target", &[9])],
        ),
        sample(
            "IntegerBinary",
            Instruction::IntegerBinary {
                operation: IntegerOperation::Add,
                destination: 1,
                left: 2,
                right: 3,
                target: 9,
            },
            &[
                ("destination", &[1]),
                ("left", &[2]),
                ("right", &[3]),
                ("target", &[9]),
            ],
        ),
        sample(
            "AllocateInstance",
            Instruction::AllocateInstance { destination: 1 },
            &[("destination", &[1])],
        ),
        sample(
            "InitializeInstance",
            Instruction::InitializeInstance {
                instance: 2,
                destination: 1,
            },
            &[("instance", &[2]), ("destination", &[1])],
        ),
        sample(
            "CheckAppliedClause",
            Instruction::CheckAppliedClause { steps: 1, selector },
            &[],
        ),
        sample(
            "InitializeSuperclass",
            Instruction::InitializeSuperclass {
                steps: 1,
                selector,
                argument_base: 20,
                argument_sources: vec![4, 5],
                destination: 6,
            },
            &[("argument_sources", &[4, 5]), ("destination", &[6])],
        ),
        sample(
            "InitializeAppliedLevel",
            Instruction::InitializeAppliedLevel {
                steps: 1,
                argument_base: 20,
                argument_sources: vec![4, 5],
                destination: 6,
            },
            &[("argument_sources", &[4, 5]), ("destination", &[6])],
        ),
    ]
}

/// The values of the fields a cell names, in the order it names them;
/// `-` names none.
fn named_values(sample: &Sample, cell: &str) -> Vec<u32> {
    if cell == "-" {
        return Vec::new();
    }
    cell.split(',')
        .flat_map(|field| {
            sample
                .fields
                .iter()
                .find(|(name, _)| *name == field)
                .unwrap_or_else(|| {
                    panic!(
                        "the row of {} names `{field}`, which the instruction has no register or target field of",
                        sample.name
                    )
                })
                .1
                .clone()
        })
        .collect()
}

fn yes_or_no(cell: &str, name: &str, column: &str) -> bool {
    match cell {
        "yes" => true,
        "no" => false,
        other => panic!("the row of {name} has `{other}` in `{column}`, not yes or no"),
    }
}

// spec: bytecode.register-effects
#[test]
fn every_instruction_has_the_register_effects_the_book_gives() {
    let table = bytecode_table("register-effects");
    let (header, rows) = table.split_first().expect("the table has a header row");
    assert_eq!(
        header,
        &[
            "instruction",
            "reads",
            "writes",
            "target",
            "falls-through",
            "outer-reach",
            "own-slots"
        ],
        "the table's columns"
    );
    let rows: BTreeMap<&str, &[String]> = rows
        .iter()
        .map(|row| {
            assert_eq!(row.len(), 7, "a row of seven cells: {row:?}");
            (row[0].as_str(), &row[1..])
        })
        .collect();
    let samples = every_sample();
    let kinds: std::collections::BTreeSet<_> = samples
        .iter()
        .map(|sample| instruction_name(&sample.instruction))
        .collect();
    assert_eq!(
        kinds.len(),
        INSTRUCTION_KINDS,
        "a sample of every kind of instruction"
    );
    assert_eq!(
        rows.keys().copied().collect::<Vec<_>>(),
        {
            let mut names: Vec<_> = samples.iter().map(|sample| sample.name).collect();
            names.sort_unstable();
            names
        },
        "one row per instruction"
    );
    for sample in &samples {
        let variant = instruction_name(&sample.instruction);
        assert_eq!(
            sample.name.split('/').next(),
            Some(variant),
            "the sample's name is its variant's"
        );
        let cells = rows[sample.name];
        let instruction = &sample.instruction;
        assert_eq!(
            registers_read(instruction)
                .map(u32::from)
                .collect::<Vec<_>>(),
            named_values(sample, &cells[0]),
            "{}: reads",
            sample.name
        );
        assert_eq!(
            register_written(instruction).map(u32::from),
            named_values(sample, &cells[1]).first().copied(),
            "{}: writes",
            sample.name
        );
        assert_eq!(
            jump_target(instruction),
            named_values(sample, &cells[2]).first().copied(),
            "{}: target",
            sample.name
        );
        assert_eq!(
            falls_through(instruction),
            yes_or_no(&cells[3], sample.name, "falls-through"),
            "{}: falls through",
            sample.name
        );
        let reach = outer_reach_of(std::slice::from_ref(instruction));
        let expected_reach = match cells[4].as_str() {
            "0" => 0,
            "depth" => match instruction {
                Instruction::LoadOuterLocal { depth, .. }
                | Instruction::StoreOuterLocal { depth, .. } => *depth,
                _ => panic!("{}: `depth` names no depth", sample.name),
            },
            "block-1" => match instruction {
                Instruction::MakeClosure { code, .. } => code.outer_reach - 1,
                _ => panic!("{}: `block-1` names no block", sample.name),
            },
            other => panic!("{}: outer reach `{other}`", sample.name),
        };
        assert_eq!(reach, expected_reach, "{}: outer reach", sample.name);
        assert_eq!(
            addresses_own_slots_of(std::slice::from_ref(instruction)),
            yes_or_no(&cells[5], sample.name, "own-slots"),
            "{}: own slots",
            sample.name
        );
    }
}
