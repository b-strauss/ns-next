#![expect(
    clippy::cast_possible_truncation,
    reason = "a test's values are small literals and lengths it built"
)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::*;
use crate::builtins::primitive::MirrorPrimitive;
use crate::bytecode::{CallSite, CompiledCode, Literal, jump_target, register_written, successors};
use crate::{Lowering, Symbol};

fn send(argument_base: u16, argument_sources: Vec<u16>, destination: u16) -> Instruction {
    Instruction::SendPublic {
        selector: Symbol::intern("m"),
        receiver_register: 0,
        argument_base,
        argument_sources,
        destination,
        call_site: CallSite::UNCACHED,
    }
}

fn load(destination: u16) -> Instruction {
    Instruction::LoadLiteral {
        destination,
        literal: Literal::Nil,
    }
}

/// Loads into every register of `registers`, which cleans them.
fn loads(registers: std::ops::Range<u16>) -> Vec<Instruction> {
    registers.map(load).collect()
}

fn branch_if_false(target: u32) -> Instruction {
    Instruction::BranchIfFalse {
        register: 0,
        target,
    }
}

/// The registers of `ranges`, one by one.
fn flattened(ranges: &[std::ops::Range<u16>]) -> Vec<u16> {
    ranges.iter().flat_map(|range| range.clone()).collect()
}

/// Ranges of registers, each `(start, end)`.
fn spans(pairs: &[(u16, u16)]) -> Vec<std::ops::Range<u16>> {
    pairs.iter().map(|&(start, end)| start..end).collect()
}

/// The registers tainted before `program_counter` of a body `check`
/// accepts.
fn tainted(
    instructions: &[Instruction],
    local_count: u16,
    register_count: u16,
    program_counter: u32,
) -> Vec<u16> {
    if let Err(refusal) = check(instructions, local_count, register_count) {
        panic!("refused: {refusal:?}");
    }
    let answers = tainted_at(
        instructions,
        local_count,
        register_count,
        &[program_counter],
    );
    assert_eq!(answers.len(), 1);
    flattened(&answers[0])
}

#[test]
fn scratch_is_tainted_at_entry() {
    let body = [Instruction::Return { register: 7 }];
    assert_eq!(
        check(&body, 6, 10),
        Err(LeftoverRefusal::Read {
            instruction: 0,
            register: 7
        })
    );
    let mut body = loads(6..10);
    body.push(Instruction::Return { register: 7 });
    assert_eq!(tainted(&body, 6, 10, 0), vec![6, 7, 8, 9]);
    assert_eq!(tainted(&body, 6, 10, 2), vec![8, 9]);
    assert_eq!(tainted(&body, 6, 10, 4), Vec::<u16>::new());
}

#[test]
fn a_send_taints_from_its_slot_end_and_writes_none_of_its_slots() {
    let mut body = loads(10..14);
    // Its slot is r10, so it taints r11 and up; its destination is r10.
    body.push(send(10, vec![10], 10));
    // Consecutive named sources are read where they stand, and a primitive
    // answers without a push: slots r10 and r11 are never written, so r10
    // stays clean and r11 tainted.
    body.push(send(10, vec![0, 1], 12));
    body.push(Instruction::Return { register: 10 });
    assert_eq!(tainted(&body, 5, 14, 4), vec![5, 6, 7, 8, 9]);
    assert_eq!(tainted(&body, 5, 14, 5), vec![5, 6, 7, 8, 9, 11, 12, 13]);
    assert_eq!(tainted(&body, 5, 14, 6), vec![5, 6, 7, 8, 9, 11, 13]);
    let mut refused = body.clone();
    refused[6] = Instruction::Return { register: 11 };
    assert_eq!(
        check(&refused, 5, 14),
        Err(LeftoverRefusal::Read {
            instruction: 6,
            register: 11
        })
    );
}

#[test]
fn a_mirror_send_taints_nothing() {
    let mut body = loads(5..10);
    body.push(Instruction::SendMirror {
        primitive: MirrorPrimitive::from_index(0),
        mirror_register: 0,
        subject_register: 1,
        argument_base: 5,
        argument_sources: vec![5],
        destination: 5,
        failure: 7,
    });
    body.push(Instruction::Return { register: 5 });
    body.push(Instruction::Return { register: 6 });
    assert_eq!(tainted(&body, 5, 10, 6), Vec::<u16>::new());
    assert_eq!(tainted(&body, 5, 10, 7), Vec::<u16>::new());
}

#[test]
fn named_locals_are_never_tainted() {
    let mut body = loads(6..9);
    body.push(send(2, vec![2], 2));
    body.push(Instruction::Return { register: 2 });
    assert_eq!(tainted(&body, 6, 9, 0), vec![6, 7, 8]);
    assert_eq!(tainted(&body, 6, 9, 3), Vec::<u16>::new());
    assert_eq!(tainted(&body, 6, 9, 4), vec![6, 7, 8]);
}

#[test]
fn a_join_is_tainted_where_either_arm_is() {
    let mut body = loads(5..10);
    body.extend([
        // Taints r6 and up, cleans r5.
        send(5, vec![5], 5),
        Instruction::BranchIfFalse {
            register: 5,
            target: 9,
        },
        // One arm sends with its slot at r6, so taints r7 and up and
        // cleans r6.
        send(6, vec![0], 6),
        Instruction::Jump { target: 10 },
        // The other writes r7, which the first send had tainted.
        load(7),
        Instruction::Return { register: 5 },
    ]);
    assert_eq!(tainted(&body, 5, 10, 8), vec![7, 8, 9]);
    assert_eq!(tainted(&body, 5, 10, 9), vec![6, 7, 8, 9]);
    assert_eq!(tainted(&body, 5, 10, 10), vec![6, 7, 8, 9]);
}

#[test]
fn a_read_a_join_brings_back_is_refused() {
    // Tainted on one arm only: the join keeps it tainted.
    let mut body = loads(5..10);
    body.extend([
        branch_if_false(7),
        send(5, vec![0], 5),
        Instruction::Return { register: 7 },
    ]);
    assert_eq!(
        check(&body, 5, 10),
        Err(LeftoverRefusal::Read {
            instruction: 7,
            register: 7
        })
    );
}

#[test]
fn crossing_forward_jumps_join_exactly() {
    let mut body = loads(4..8);
    body.extend([
        // 4: carries the clean state to 8.
        branch_if_false(8),
        // 5: taints r5 to r7.
        send(4, vec![0], 4),
        // 6: carries r5 to r7 over 8, to 10.
        branch_if_false(10),
        // 7
        load(5),
        // 8: the clean state joins r6 and r7.
        load(6),
        // 9
        load(7),
        // 10: r5 to r7 join the clean state.
        Instruction::Move {
            destination: 4,
            source: 1,
        },
        Instruction::Return { register: 0 },
    ]);
    assert_eq!(tainted(&body, 4, 8, 8), vec![6, 7]);
    assert_eq!(tainted(&body, 4, 8, 9), vec![7]);
    assert_eq!(tainted(&body, 4, 8, 10), vec![5, 6, 7]);
    assert_eq!(tainted(&body, 4, 8, 11), vec![5, 6, 7]);
    if let Err(message) = agrees_with_the_oracle(&body, 4, 8) {
        panic!("{message}");
    }
}

#[test]
fn a_loop_head_takes_the_floor() {
    let body = [
        load(5),
        load(7),
        load(8),
        load(9),
        // 4: the head; r6 is tainted since entry.
        branch_if_false(8),
        // 5: the loop's least slot end is r8.
        send(7, vec![0], 7),
        load(8),
        // 7: the back edge.
        Instruction::Jump { target: 4 },
        Instruction::Return { register: 0 },
    ];
    assert_eq!(tainted(&body, 5, 10, 4), vec![6, 8, 9]);
    assert_eq!(tainted(&body, 5, 10, 7), vec![6, 9]);
    assert_eq!(tainted(&body, 5, 10, 8), vec![6, 8, 9]);
    // A loop without a send adds nothing at its head.
    let body = [
        load(5),
        branch_if_false(4),
        load(6),
        Instruction::Jump { target: 1 },
        Instruction::Return { register: 5 },
    ];
    assert_eq!(tainted(&body, 5, 8, 1), vec![6, 7]);
}

#[test]
fn a_jump_into_a_loop_past_its_head_is_past_the_bound() {
    let body = [
        branch_if_false(2),
        load(5),
        send(6, vec![0], 6),
        Instruction::BranchIfTrue {
            register: 0,
            target: 1,
        },
        Instruction::Return { register: 0 },
    ];
    assert_eq!(
        check(&body, 5, 10),
        Err(LeftoverRefusal::PastTheBound { instruction: 0 })
    );
    // Into the inner of two nested loops, from inside the outer one.
    let body = [
        load(5),
        // 1: the outer head.
        load(6),
        branch_if_false(4),
        // 3: the inner head.
        load(7),
        // 4: past the inner head, from 2.
        load(8),
        Instruction::BranchIfTrue {
            register: 0,
            target: 3,
        },
        Instruction::BranchIfTrue {
            register: 0,
            target: 1,
        },
        Instruction::Return { register: 0 },
    ];
    assert_eq!(
        check(&body, 5, 10),
        Err(LeftoverRefusal::PastTheBound { instruction: 2 })
    );
}

#[test]
fn crossing_loops_are_past_the_bound() {
    let body = [
        load(5),
        load(6),
        Instruction::BranchIfTrue {
            register: 0,
            target: 0,
        },
        Instruction::BranchIfTrue {
            register: 0,
            target: 1,
        },
        Instruction::Return { register: 0 },
    ];
    assert!(matches!(
        check(&body, 5, 10),
        Err(LeftoverRefusal::PastTheBound { .. })
    ));
}

/// `count` forward branches, each to a target of its own, nested, then the
/// targets.
fn nested_branches(count: u32) -> Vec<Instruction> {
    let mut body: Vec<Instruction> = (0..count)
        .map(|index| branch_if_false(2 * count - 1 - index))
        .collect();
    body.extend((0..count).map(|_| load(1)));
    body.push(Instruction::Return { register: 0 });
    body
}

#[test]
fn more_than_257_pending_targets_are_past_the_bound() {
    assert_eq!(PENDING_LIMIT, 257);
    let limit = PENDING_LIMIT as u32;
    assert_eq!(
        check(&nested_branches(limit + 1), 5, 10),
        Err(LeftoverRefusal::PastTheBound { instruction: limit })
    );
    assert_eq!(check(&nested_branches(limit), 5, 10), Ok(()));
    // Jumps to one target share its pending state.
    let mut body: Vec<Instruction> = (0..2 * limit).map(|_| branch_if_false(2 * limit)).collect();
    body.push(Instruction::Return { register: 0 });
    assert_eq!(check(&body, 5, 10), Ok(()));
}

#[test]
fn a_load_that_raises_ends_its_path() {
    // The body a body past a limit compiles to: its load raises, so its
    // return, which reads a scratch register nothing wrote, never runs.
    let body = [
        Instruction::LoadLiteral {
            destination: 15,
            literal: Literal::BodyTooLarge(crate::bytecode::BodyLimit::Arguments),
        },
        Instruction::ImplicitReturn { register: 15 },
    ];
    assert_eq!(tainted(&body, 15, 16, 0), vec![15]);
    assert_eq!(tainted(&body, 15, 16, 1), Vec::<u16>::new());
}

#[test]
fn the_most_a_body_holds_is_258_bitsets_of_the_widest_register_file() {
    let analysis = match analyze(&nested_branches(PENDING_LIMIT as u32), 5, u16::MAX, &[]) {
        Ok(analysis) => analysis,
        Err(refusal) => panic!("refused: {refusal:?}"),
    };
    let spent = analysis.spent;
    eprintln!(
        "{} bitsets, {} bytes, {} word operations per instruction",
        spent.peak_bitsets, spent.peak_bytes, spent.most_word_operations
    );
    assert_eq!(spent.words, 1024);
    assert_eq!(spent.peak_pending, PENDING_LIMIT);
    assert_eq!(spent.peak_bitsets, PENDING_LIMIT + 1);
    assert_eq!(spent.peak_bytes, 258 * 1024 * 8);
    assert!(spent.peak_bytes as f64 <= 2.02 * f64::from(1u32 << 20));
    assert!(spent.most_word_operations <= 3 * spent.words as u64);
}

#[test]
fn a_body_past_the_instruction_limit_is_past_the_bound() {
    let body = vec![Instruction::Return { register: 0 }; INSTRUCTION_LIMIT as usize + 1];
    assert_eq!(
        check(&body, 5, 10),
        Err(LeftoverRefusal::PastTheBound {
            instruction: INSTRUCTION_LIMIT
        })
    );
}

#[test]
fn tainted_at_answers_each_program_counter() {
    let mut body = loads(5..9);
    body.extend([
        // 4
        send(6, vec![0], 6),
        // 5
        Instruction::Jump { target: 7 },
        // 6: unreachable.
        load(5),
        // 7
        load(8),
        Instruction::Return { register: 5 },
    ]);
    assert_eq!(check(&body, 5, 9), Ok(()));
    let answers = tainted_at(&body, 5, 9, &[0, 4, 5, 5, 6, 7, 8]);
    assert_eq!(
        answers,
        vec![
            spans(&[(5, 9)]),
            spans(&[]),
            spans(&[(7, 9)]),
            spans(&[(7, 9)]),
            spans(&[]),
            spans(&[(7, 9)]),
            spans(&[(7, 8)]),
        ]
    );
    for (program_counter, answer) in [0u32, 4, 5, 6, 7, 8].into_iter().zip([
        spans(&[(5, 9)]),
        spans(&[]),
        spans(&[(7, 9)]),
        spans(&[]),
        spans(&[(7, 9)]),
        spans(&[(7, 8)]),
    ]) {
        assert_eq!(tainted_at(&body, 5, 9, &[program_counter]), vec![answer]);
    }
    assert_eq!(tainted_at(&body, 5, 9, &[]), Vec::<Vec<_>>::new());
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "a program counter past a body")]
fn a_program_counter_past_the_body_is_refused() {
    let body = [Instruction::Return { register: 0 }];
    tainted_at(&body, 5, 9, &[1]);
}

/// `count` loops, each nested in the one after it: the heads first, then
/// the back edges, innermost first.
fn nested_loops(count: u32) -> Vec<Instruction> {
    let mut body: Vec<Instruction> = (0..count).map(|_| load(1)).collect();
    body.extend((0..count).map(|index| Instruction::BranchIfTrue {
        register: 0,
        target: count - 1 - index,
    }));
    body.push(Instruction::Return { register: 0 });
    body
}

#[test]
fn more_than_128_open_loops_are_past_the_bound() {
    assert_eq!(LOOP_LIMIT, 128);
    let limit = LOOP_LIMIT as u32;
    assert_eq!(
        check(&nested_loops(limit + 1), 5, 10),
        Err(LeftoverRefusal::PastTheBound { instruction: limit })
    );
    match analyze(&nested_loops(limit), 5, 10, &[]) {
        Ok(analysis) => assert_eq!(analysis.spent.peak_open_loops, LOOP_LIMIT),
        Err(refusal) => panic!("refused: {refusal:?}"),
    }
}

#[test]
fn the_bitsets_cross_word_boundaries() {
    let mut body = loads(3..200);
    // Taints r63 to r199 in a body of 200 registers.
    body.push(send(62, vec![0], 62));
    body.push(load(64));
    body.push(load(127));
    body.push(load(128));
    body.push(Instruction::Return { register: 0 });
    let answers = tainted_at(&body, 3, 200, &[198, 200, 201]);
    assert_eq!(answers[0], spans(&[(63, 200)]));
    assert_eq!(answers[1], spans(&[(63, 64), (65, 127), (128, 200)]));
    assert_eq!(answers[2], spans(&[(63, 64), (65, 127), (129, 200)]));
}

/// The analysis as a plain fixpoint over one explicit set of tainted
/// registers per instruction: quadratic, and obviously what the bitsets
/// stand for. A loop's head is where the analysis may say more than this;
/// everywhere else the two agree.
fn oracle(
    instructions: &[Instruction],
    local_count: u16,
    register_count: u16,
) -> Vec<Option<BTreeSet<u16>>> {
    let local_count = local_count.min(register_count);
    let count = instructions.len();
    let mut states: Vec<Option<BTreeSet<u16>>> = vec![None; count];
    if count == 0 {
        return states;
    }
    states[0] = Some((local_count..register_count).collect());
    let mut worklist = vec![0usize];
    while let Some(index) = worklist.pop() {
        let instruction = &instructions[index];
        let mut state = states[index].clone().expect("a visited state");
        if let Some((base, sources)) = instruction.argument_slots() {
            let end = (usize::from(base) + sources.len()).min(usize::from(register_count)) as u16;
            if !matches!(instruction, Instruction::SendMirror { .. }) {
                for register in end.max(local_count)..register_count {
                    state.insert(register);
                }
            }
        }
        let edge = state.clone();
        if let Some(destination) = register_written(instruction) {
            state.remove(&destination);
        }
        let mut flow = |target: usize, incoming: &BTreeSet<u16>, worklist: &mut Vec<usize>| {
            if target >= count {
                return;
            }
            let changed = match &mut states[target] {
                Some(existing) => {
                    let before = existing.len();
                    existing.extend(incoming.iter().copied());
                    existing.len() != before
                }
                slot @ None => {
                    *slot = Some(incoming.clone());
                    true
                }
            };
            if changed {
                worklist.push(target);
            }
        };
        if falls_through(instruction) {
            flow(index + 1, &state, &mut worklist);
        }
        if let Some(target) = jump_target(instruction) {
            flow(target as usize, &edge, &mut worklist);
        }
    }
    states
}

/// The first instruction a backward jump targets; past the body for none.
/// Before it, no state depends on a loop.
fn first_loop_head(instructions: &[Instruction]) -> usize {
    instructions
        .iter()
        .enumerate()
        .flat_map(|(index, instruction)| {
            successors(index as u32, instruction)
                .map(|target| target as usize)
                .filter(move |target| *target <= index)
        })
        .min()
        .unwrap_or(instructions.len())
}

/// Holds the analysis to the oracle at every instruction of `instructions`,
/// a body `check` accepted: equal before the first loop head, never less
/// from there.
fn agrees_with_the_oracle(
    instructions: &[Instruction],
    local_count: u16,
    register_count: u16,
) -> Result<(), String> {
    let expected = oracle(instructions, local_count, register_count);
    let first_head = first_loop_head(instructions);
    // Every program counter from one pass; the last one asked alone as
    // well.
    let program_counters: Vec<u32> = (0..instructions.len() as u32).collect();
    let every = tainted_at(instructions, local_count, register_count, &program_counters);
    if let Some(&last) = program_counters.last() {
        let single = tainted_at(instructions, local_count, register_count, &[last]);
        if every.last() != single.first() {
            return Err(format!(
                "at {last}: one query answers {single:?}, the pass {:?}",
                every.last()
            ));
        }
    }
    for (index, expected) in expected.iter().enumerate() {
        let ranges = &every[index];
        let sorted_and_disjoint = ranges.windows(2).all(|pair| pair[0].end < pair[1].start)
            && ranges.iter().all(|range| range.start < range.end);
        if !sorted_and_disjoint {
            return Err(format!("at {index}: ranges {ranges:?} overlap or touch"));
        }
        let found: BTreeSet<u16> = flattened(ranges).into_iter().collect();
        let expected = expected.clone().unwrap_or_default();
        let agrees = if index >= first_head {
            found.is_superset(&expected)
        } else {
            found == expected
        };
        if !agrees {
            return Err(format!(
                "at {index}: found {found:?}, the oracle {expected:?}"
            ));
        }
    }
    Ok(())
}

/// A small deterministic generator for crafted bodies.
struct Generator(u64);

impl Generator {
    fn next(&mut self, bound: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % u64::from(bound)) as u32
    }
}

/// A crafted body that reads only named registers, so that it is never
/// refused for a read and the states can be compared: sends, writes and
/// forward jumps that cross as they please, and now and then a loop.
fn crafted_body(generator: &mut Generator, length: u32, register_count: u16) -> Vec<Instruction> {
    let local_count = 4u16;
    let scratch = |generator: &mut Generator| {
        local_count + generator.next(u32::from(register_count - local_count)) as u16
    };
    let mut body = Vec::new();
    let mut index = 0u32;
    while index + 1 < length {
        let instruction = match generator.next(9) {
            0 | 1 => {
                let base = scratch(generator);
                let arguments = generator.next(3) as u16;
                let arguments = arguments.min(register_count - base);
                let sources: Vec<u16> = (0..arguments).map(|offset| offset % local_count).collect();
                send(base, sources, scratch(generator))
            }
            2 | 3 => load(scratch(generator)),
            4 | 5 => branch_if_false(index + 1 + generator.next(length - index - 1)),
            6 => Instruction::Jump {
                target: index + 1 + generator.next(length - index - 1),
            },
            7 => Instruction::BranchIfTrue {
                register: 0,
                target: index.saturating_sub(generator.next(4)),
            },
            _ => Instruction::Move {
                destination: scratch(generator),
                source: 1,
            },
        };
        body.push(instruction);
        index += 1;
    }
    body.push(Instruction::Return { register: 0 });
    body
}

#[test]
fn crafted_bodies_agree_with_the_oracle() {
    let mut generator = Generator(0x5EAD_5EAD);
    let mut accepted = 0;
    for round in 0..4000 {
        let length = 4 + generator.next(40);
        let register_count = 6 + generator.next(10) as u16;
        let body = crafted_body(&mut generator, length, register_count);
        match check(&body, 4, register_count) {
            Ok(()) => {
                accepted += 1;
                if let Err(message) = agrees_with_the_oracle(&body, 4, register_count) {
                    panic!("round {round}: {message}\n{body:#?}");
                }
            }
            Err(LeftoverRefusal::PastTheBound { .. }) => {}
            Err(refusal) => panic!("round {round}: {refusal:?}"),
        }
    }
    assert!(accepted > 1000, "only {accepted} accepted");
}

#[test]
fn crafted_bodies_of_many_registers_agree_with_the_oracle() {
    // Wider than a word, so that every bitset operation crosses words.
    let mut generator = Generator(0x00B1_75E7);
    for round in 0..300 {
        let length = 4 + generator.next(60);
        let register_count = 60 + generator.next(200) as u16;
        let body = crafted_body(&mut generator, length, register_count);
        if check(&body, 4, register_count).is_ok()
            && let Err(message) = agrees_with_the_oracle(&body, 4, register_count)
        {
            panic!("round {round}: {message}\n{body:#?}");
        }
    }
}

fn compile_first_method(source: &str, lowering: Lowering) -> CompiledCode {
    let module = match ns_syntax::parse(source) {
        Ok(module) => module,
        Err(diagnostics) => panic!("does not parse: {:?}", diagnostics.first()),
    };
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
    crate::compiler::compile_method(method, &resolved.bindings, lowering, None)
}

/// The code `code` and every block it creates, blocks first.
fn collect(code: &Rc<CompiledCode>, into: &mut Vec<Rc<CompiledCode>>) {
    for instruction in code.instructions() {
        if let Instruction::MakeClosure { code: block, .. } = instruction {
            collect(&block, into);
        }
    }
    into.push(Rc::clone(code));
}

fn collect_mixin(mixin: &Rc<crate::Mixin>, into: &mut Vec<Rc<CompiledCode>>) {
    for entry in mixin.methods.values() {
        match entry {
            crate::mixin::MethodEntry::Compiled(method) => collect(&method.code, into),
            crate::mixin::MethodEntry::Factory(factory) => collect(&factory.code, into),
            crate::mixin::MethodEntry::SlotAccessor(_)
            | crate::mixin::MethodEntry::Primitive(_) => {}
        }
    }
    for code in [
        &mixin.initializer_code,
        &mixin.own_initializer_code,
        &mixin.standalone_initializer_code,
        &mixin.superclass_clause_code,
    ]
    .into_iter()
    .flatten()
    {
        collect(code, into);
    }
    if let Some(class_side) = &mixin.class_side {
        collect_mixin(class_side, into);
    }
    for nested in mixin.declared_nested().iter() {
        collect_mixin(nested, into);
    }
}

fn sources_under(directory: &Path, into: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(directory)
        .expect("a directory")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            sources_under(&path, into);
        } else if path.extension().is_some_and(|extension| extension == "ns") {
            into.push(path);
        }
    }
}

/// The largest figures the pass reached over a set of bodies, each with the
/// body it was reached on.
#[derive(Default)]
struct Largest {
    bytes: (u64, String),
    bitsets: (usize, String),
    pending: (usize, String),
    open_loops: (usize, String),
    word_operations: (u64, String),
    /// Word operations per instruction against the words of a bitset.
    operations_per_word: (f64, String),
}

impl Largest {
    fn record(&mut self, spent: &Spent, label: &str) {
        if spent.peak_bytes > self.bytes.0 {
            self.bytes = (spent.peak_bytes, label.to_string());
        }
        if spent.peak_bitsets > self.bitsets.0 {
            self.bitsets = (spent.peak_bitsets, label.to_string());
        }
        if spent.peak_pending > self.pending.0 {
            self.pending = (spent.peak_pending, label.to_string());
        }
        if spent.peak_open_loops > self.open_loops.0 {
            self.open_loops = (spent.peak_open_loops, label.to_string());
        }
        if spent.most_word_operations > self.word_operations.0 {
            self.word_operations = (spent.most_word_operations, label.to_string());
        }
        let per_word = spent.most_word_operations as f64 / spent.words.max(1) as f64;
        if per_word > self.operations_per_word.0 {
            self.operations_per_word = (per_word, label.to_string());
        }
    }

    fn report(&self, what: &str) {
        eprintln!(
            "{what}: largest memory {} bytes ({}), most bitsets {} ({}), most pending {} ({}), most open loops {} ({}), most word operations per instruction {} ({}), per word of a bitset {:.2} ({})",
            self.bytes.0,
            self.bytes.1,
            self.bitsets.0,
            self.bitsets.1,
            self.pending.0,
            self.pending.1,
            self.open_loops.0,
            self.open_loops.1,
            self.word_operations.0,
            self.word_operations.1,
            self.operations_per_word.0,
            self.operations_per_word.1,
        );
    }
}

/// Runs the pass over `code` and holds it to the bound: at most
/// `PENDING_LIMIT + 1` bitsets, and at most three bitset operations per
/// instruction.
fn measure(code: &CompiledCode) -> Result<Spent, String> {
    let instructions = code.instructions();
    let spent = analyze(&instructions, code.local_count, code.register_count, &[])
        .map(|analysis| analysis.spent)
        .map_err(|refusal| format!("{refusal:?}"))?;
    assert!(spent.peak_pending <= PENDING_LIMIT);
    assert!(spent.peak_open_loops <= LOOP_LIMIT);
    assert!(spent.peak_bitsets <= PENDING_LIMIT + 1);
    assert!(spent.peak_bytes <= ((PENDING_LIMIT + 1) * spent.words * 8) as u64);
    assert!(spent.most_word_operations <= 3 * spent.words as u64);
    Ok(spent)
}

#[test]
fn every_golden_body_stays_within_the_bound() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    sources_under(&root.join("goldens"), &mut files);
    sources_under(&root.join("benchmarks"), &mut files);
    let mut bodies = 0usize;
    let mut modules = 0usize;
    let mut largest = Largest::default();
    let mut failures = Vec::new();
    let mut skipped: Vec<PathBuf> = Vec::new();
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let mut resolved_modules: Vec<(String, Rc<crate::ResolvedModule>)> = Vec::new();
        for file in &files {
            let source = std::fs::read_to_string(file).expect("readable");
            let resolved = ns_syntax::parse(&source).ok().and_then(|module| {
                crate::resolve(
                    module,
                    Rc::from(source.as_str()),
                    crate::SourceOrigin::File(file.clone()),
                )
                .ok()
            });
            match resolved {
                Some(resolved) => {
                    resolved_modules.push((file.display().to_string(), Rc::new(resolved)))
                }
                None => {
                    if lowering == Lowering::Optimized {
                        skipped.push(file.clone());
                    }
                }
            }
        }
        resolved_modules.push((
            "Kernel".to_string(),
            crate::interpreter::kernel::resolved_kernel(),
        ));
        for shipped in crate::interpreter::core_library::SHIPPED_MODULES {
            resolved_modules.push((
                shipped.name.to_string(),
                crate::interpreter::core_library::resolved_shipped_module(shipped.name),
            ));
        }
        for (name, resolved) in &resolved_modules {
            modules += 1;
            let mixin = crate::Interpreter::bootstrap_with(lowering).compile_module(resolved);
            let mut codes = Vec::new();
            collect_mixin(&mixin, &mut codes);
            for code in codes {
                bodies += 1;
                let instructions = code.instructions();
                let label = format!(
                    "{name} {lowering:?} ({} instructions, {} registers)",
                    instructions.len(),
                    code.register_count
                );
                match measure(&code) {
                    Ok(spent) => {
                        largest.record(&spent, &label);
                        if instructions.len() <= 400
                            && let Err(message) = agrees_with_the_oracle(
                                &instructions,
                                code.local_count,
                                code.register_count,
                            )
                        {
                            failures.push(format!("{label}: {message}"));
                        }
                    }
                    Err(refusal) => failures.push(format!(
                        "{label}: {refusal}\n{}",
                        crate::bytecode::disassemble(&code, "body")
                    )),
                }
            }
        }
    }
    eprintln!("{modules} modules, {bodies} bodies");
    largest.report("every compiled body");
    for failure in failures.iter().take(3) {
        eprintln!("{failure}");
    }
    assert!(failures.is_empty(), "{} bodies failed", failures.len());
    // A source that does not parse or resolve is a golden that records
    // that error, never a benchmark.
    for file in &skipped {
        let outcome = std::fs::read_to_string(file.with_extension("toml")).unwrap_or_default();
        assert!(
            outcome.contains("status = 1")
                && (outcome.contains("parse error") || outcome.contains("resolve error")),
            "{} neither compiles nor records a parse or resolve error",
            file.display()
        );
    }
    eprintln!(
        "{} sources skipped, each a golden of a parse or resolve error",
        skipped.len()
    );
    assert!(bodies > 10_000, "only {bodies} bodies");
}

fn method_source(body: &str) -> String {
    format!("class Shapes = () (\n  run: a with: b = (\n    {body}\n  )\n) : ()\n")
}

/// The flat worst case of a state stored per jump target: one tuple of
/// `width` elements, each an inlined conditional over a value an inner send
/// left a tainted register beside. A tuple, since a send passes no more
/// than `ARGUMENT_LIMIT` arguments.
fn flat(width: usize) -> String {
    let element = "((a f: (b g: a)) ifTrue: [a] ifFalse: [b])";
    let elements = vec![element; width].join(". ");
    method_source(&format!("^{{b. {elements}}}"))
}

/// Compiles `source`'s method under both lowerings and holds it, and every
/// block it creates, to the bound; answers the most pending targets.
fn assert_within_the_bound(source: &str, what: &str) -> usize {
    let mut most_pending = 0;
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let mut codes = Vec::new();
        collect(&Rc::new(compile_first_method(source, lowering)), &mut codes);
        let mut largest = Largest::default();
        let mut instructions = 0;
        for code in &codes {
            instructions += code.instruction_count();
            let spent = match measure(code) {
                Ok(spent) => spent,
                Err(refusal) => panic!(
                    "{what} {lowering:?}: {refusal}
{}",
                    crate::bytecode::disassemble(code, "body")
                ),
            };
            largest.record(
                &spent,
                &format!("{} instructions", code.instruction_count()),
            );
        }
        largest.report(&format!(
            "{what} {lowering:?}, {} bodies, {instructions} instructions",
            codes.len()
        ));
        most_pending = most_pending.max(largest.pending.0);
    }
    most_pending
}

#[test]
fn the_flat_worst_case_stays_within_the_bound() {
    for width in [500, 2000] {
        let source = flat(width);
        for lowering in [Lowering::Optimized, Lowering::Reference] {
            let code = compile_first_method(&source, lowering);
            assert!(
                code.instruction_count() > width,
                "flat {width} {lowering:?} compiled to {} instructions",
                code.instruction_count()
            );
        }
        assert_within_the_bound(&source, &format!("flat {width}"));
    }
}

/// `depth` inlined forms of `shape`, each nested in the block of the one
/// around it, `innermost` at the bottom; `shape` writes `{inner}` where the
/// nested one goes.
fn nested(depth: usize, shape: &str, innermost: &str) -> String {
    let mut expression = innermost.to_string();
    for _ in 0..depth {
        expression = shape.replace("{inner}", &expression);
    }
    method_source(&format!("^{expression}"))
}

/// The deepest nesting of `shape` the parser accepts.
fn deepest(shape: &str, innermost: &str) -> usize {
    let (mut low, mut high) = (1usize, 4 * PARSER_NESTING_LIMIT);
    while low < high {
        let middle = (low + high).div_ceil(2);
        if ns_syntax::parse(&nested(middle, shape, innermost)).is_ok() {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

/// Each shape, and whether the parser's limit is the depth to build it at.
/// An inlined conditional's fallback closure holds its blocks compiled
/// again, so the code of nested conditionals doubles with each level and
/// they are built only a dozen deep; a loop and an integer operation have
/// no fallback closure, and are built at the parser's limit.
const SHAPES: &[(&str, bool)] = &[
    ("[a f: b] whileTrue: [b g: a. {inner}]", true),
    ("([a f: b] whileTrue: [b g: a. {inner}])", true),
    ("([a f: b] whileTrue: [b g: (a f: {inner})])", true),
    ("((a f: b) + ({inner}))", true),
    ("((b g: a) + ([a] whileFalse: [{inner}]))", true),
    (
        "(a ifTrue: [b g: (a f: {inner})] ifFalse: [a f: (b g: {inner})])",
        false,
    ),
    ("(a ifTrue: [b g: a. {inner}])", false),
    ("(a and: [(b g: a) or: [{inner}]])", false),
    (
        "([a f: b] whileTrue: [a ifTrue: [{inner}] ifFalse: [b g: a]])",
        false,
    ),
];

#[test]
fn the_deepest_nesting_stays_within_the_bound() {
    let innermost = "(a f: (b g: a))";
    for &(shape, at_the_limit) in SHAPES {
        let depth = if at_the_limit {
            deepest(shape, innermost)
        } else {
            8 / shape.matches("{inner}").count().max(1) + 2
        };
        let pending = assert_within_the_bound(
            &nested(depth, shape, innermost),
            &format!("{shape} x{depth}"),
        );
        assert!(pending <= PENDING_LIMIT, "{shape}: {pending} pending");
    }
}

#[test]
fn the_parser_accepts_no_deeper_nesting() {
    // A block nested in a block: each is one of the parser's guarded
    // productions, which is what `PARSER_NESTING_LIMIT` counts.
    let blocks =
        |depth: usize| method_source(&format!("^{}a{}", "[".repeat(depth), "]".repeat(depth)));
    let mut deepest = 0;
    for depth in 1..=4 * PARSER_NESTING_LIMIT {
        if ns_syntax::parse(&blocks(depth)).is_ok() {
            deepest = depth;
        }
    }
    assert!(deepest > 0);
    assert!(
        deepest <= PARSER_NESTING_LIMIT,
        "the parser accepts {deepest} nested blocks; raise PARSER_NESTING_LIMIT and re-derive the bound"
    );
}
