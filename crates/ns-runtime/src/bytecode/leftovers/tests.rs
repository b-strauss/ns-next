use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::*;
use crate::builtins::primitive::MirrorPrimitive;
use crate::bytecode::{
    falls_through, jump_target, register_written, successors, CallSite, CompiledCode, Literal,
};
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

fn tainted(leftovers: &Leftovers, program_counter: u32) -> Vec<u16> {
    leftovers.tainted_before(program_counter).collect()
}

fn analyzed(instructions: &[Instruction], local_count: u16, register_count: u16) -> Leftovers {
    match analyze(instructions, local_count, register_count) {
        Ok(leftovers) => leftovers,
        Err(refusal) => panic!("refused: {refusal:?}"),
    }
}

#[test]
fn a_send_taints_the_registers_from_its_slot_end() {
    let body = [send(5, vec![5, 6], 5), Instruction::Return { register: 5 }];
    let leftovers = analyzed(&body, 5, 10);
    assert_eq!(tainted(&leftovers, 0), Vec::<u16>::new());
    assert_eq!(tainted(&leftovers, 1), vec![7, 8, 9]);
}

#[test]
fn a_write_cleans_a_register() {
    let body = [
        send(5, vec![5, 6], 5),
        load(8),
        Instruction::Return { register: 8 },
    ];
    let leftovers = analyzed(&body, 5, 10);
    assert_eq!(tainted(&leftovers, 2), vec![7, 9]);
}

#[test]
fn a_mirror_send_taints_nothing() {
    let body = [
        Instruction::SendMirror {
            primitive: MirrorPrimitive::from_index(0),
            mirror_register: 0,
            subject_register: 1,
            argument_base: 5,
            argument_sources: vec![5],
            destination: 5,
            failure: 2,
        },
        Instruction::Return { register: 5 },
        Instruction::Return { register: 6 },
    ];
    let leftovers = analyzed(&body, 5, 10);
    assert_eq!(tainted(&leftovers, 1), Vec::<u16>::new());
    assert_eq!(tainted(&leftovers, 2), Vec::<u16>::new());
}

#[test]
fn a_send_writes_none_of_its_slots() {
    let body = [
        load(10),
        // Its slot is r10, so it taints r11 and up.
        send(10, vec![10], 10),
        // Consecutive named sources are read where they stand, and a
        // primitive answers without a push: slots r10 and r11 are never
        // written, so r10 stays clean and r11 tainted.
        send(10, vec![0, 1], 12),
        Instruction::Return { register: 11 },
    ];
    assert_eq!(
        analyze(&body, 5, 14).err(),
        Some(LeftoverRefusal::Read {
            instruction: 3,
            register: 11
        })
    );
    let body = [
        load(10),
        send(10, vec![10], 10),
        send(10, vec![0, 1], 12),
        Instruction::Return { register: 10 },
    ];
    let leftovers = analyzed(&body, 5, 14);
    assert_eq!(tainted(&leftovers, 3), vec![11, 13]);
}

#[test]
fn named_locals_are_never_tainted() {
    let body = [send(2, vec![2], 2), Instruction::Return { register: 2 }];
    let leftovers = analyzed(&body, 6, 9);
    assert_eq!(tainted(&leftovers, 1), vec![6, 7, 8]);
}

#[test]
fn a_join_is_tainted_where_either_arm_is() {
    let body = [
        send(5, vec![5], 5),
        Instruction::BranchIfFalse {
            register: 5,
            target: 4,
        },
        // One arm sends with its slot at 6, so taints 7 and up and cleans 6.
        send(6, vec![0], 6),
        Instruction::Jump { target: 5 },
        // The other writes 7, which the first send had tainted.
        load(7),
        Instruction::Return { register: 5 },
    ];
    let leftovers = analyzed(&body, 5, 10);
    assert_eq!(tainted(&leftovers, 3), vec![7, 8, 9]);
    assert_eq!(tainted(&leftovers, 4), vec![6, 7, 8, 9]);
    assert_eq!(tainted(&leftovers, 5), vec![6, 7, 8, 9]);
}

#[test]
fn a_loop_body_taints_its_head() {
    let body = [
        load(5),
        Instruction::BranchIfFalse {
            register: 5,
            target: 4,
        },
        send(6, vec![6], 6),
        Instruction::Jump { target: 1 },
        Instruction::Return { register: 5 },
    ];
    let leftovers = analyzed(&body, 5, 10);
    let head = tainted(&leftovers, 1);
    for register in [7, 8, 9] {
        assert!(head.contains(&register), "{head:?}");
    }
    assert!(!head.contains(&5));
}

#[test]
fn a_read_of_a_tainted_register_is_refused() {
    let body = [send(5, vec![5, 6], 5), Instruction::Return { register: 8 }];
    assert_eq!(
        analyze(&body, 5, 10).err(),
        Some(LeftoverRefusal::Read {
            instruction: 1,
            register: 8
        })
    );
}

#[test]
fn a_read_after_rewriting_is_accepted() {
    let body = [
        send(5, vec![5, 6], 5),
        load(8),
        Instruction::Return { register: 8 },
    ];
    assert!(analyze(&body, 5, 10).is_ok());
}

#[test]
fn a_read_a_join_brings_back_is_refused() {
    // Tainted on one arm only: the join keeps it tainted.
    let body = [
        load(7),
        Instruction::BranchIfFalse {
            register: 5,
            target: 3,
        },
        send(5, vec![5], 5),
        Instruction::Return { register: 7 },
    ];
    assert_eq!(
        analyze(&body, 5, 10).err(),
        Some(LeftoverRefusal::Read {
            instruction: 3,
            register: 7
        })
    );
}

#[test]
fn a_crafted_body_past_the_bound_is_refused() {
    // More forward jumps open at once than any compiled body keeps.
    let open = OPEN_ENTRY_LIMIT as u32 + 1;
    let mut body: Vec<Instruction> = (0..open)
        .map(|_| Instruction::BranchIfFalse {
            register: 0,
            target: open,
        })
        .collect();
    body.push(Instruction::Return { register: 0 });
    assert_eq!(
        analyze(&body, 5, 10).err(),
        Some(LeftoverRefusal::PastTheBound {
            instruction: open - 1
        })
    );
    // At the limit, the same body is accepted.
    let mut body: Vec<Instruction> = (0..open - 1)
        .map(|_| Instruction::BranchIfFalse {
            register: 0,
            target: open - 1,
        })
        .collect();
    body.push(Instruction::Return { register: 0 });
    assert!(analyze(&body, 5, 10).is_ok());
}

#[test]
fn a_loop_entered_past_its_head_is_refused() {
    let body = [
        Instruction::BranchIfFalse {
            register: 0,
            target: 2,
        },
        load(5),
        send(6, vec![6], 6),
        Instruction::BranchIfTrue {
            register: 0,
            target: 1,
        },
        Instruction::Return { register: 0 },
    ];
    assert_eq!(
        analyze(&body, 5, 10).err(),
        Some(LeftoverRefusal::PastTheBound { instruction: 0 })
    );
}

#[test]
fn crossing_loops_are_refused() {
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
        analyze(&body, 5, 10),
        Err(LeftoverRefusal::PastTheBound { .. })
    ));
}

/// The analysis as a plain fixpoint over one explicit set of tainted
/// registers per instruction: quadratic, and obviously what the
/// representation stands for. A loop's head is where the analysis may say
/// more than this; everywhere else the two agree.
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
    states[0] = Some(BTreeSet::new());
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

fn has_back_edge(instructions: &[Instruction]) -> bool {
    instructions.iter().enumerate().any(|(index, instruction)| {
        successors(index as u32, instruction).any(|target| target as usize <= index)
    })
}

/// Holds the analysis to the oracle at every instruction of `instructions`:
/// equal without a loop, never less with one.
fn agrees_with_the_oracle(
    leftovers: &Leftovers,
    instructions: &[Instruction],
    local_count: u16,
    register_count: u16,
) -> Result<(), String> {
    let expected = oracle(instructions, local_count, register_count);
    let looping = has_back_edge(instructions);
    // Every program counter from one pass, in reverse to hold the answers
    // to the order asked; the last one through the single query as well.
    let program_counters: Vec<u32> = (0..instructions.len() as u32).rev().collect();
    let mut every = leftovers.tainted_before_each(&program_counters);
    every.reverse();
    if let Some(last) = program_counters.first() {
        let single: Vec<u16> = leftovers.tainted_before(*last).collect();
        if every.last() != Some(&single) {
            return Err(format!(
                "at {last}: one query answers {single:?}, the pass {:?}",
                every.last()
            ));
        }
    }
    for (index, expected) in expected.iter().enumerate() {
        let found: BTreeSet<u16> = every[index].iter().copied().collect();
        let expected = expected.clone().unwrap_or_default();
        let agrees = if looping {
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
            4 | 5 => Instruction::BranchIfFalse {
                register: 0,
                target: index + 1 + generator.next(length - index - 1),
            },
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
        match analyze(&body, 4, register_count) {
            Ok(leftovers) => {
                accepted += 1;
                if let Err(message) = agrees_with_the_oracle(&leftovers, &body, 4, register_count) {
                    panic!("round {round}: {message}\n{body:#?}");
                }
            }
            Err(LeftoverRefusal::PastTheBound { .. }) => {}
            Err(refusal) => panic!("round {round}: {refusal:?}"),
        }
    }
    assert!(accepted > 1000, "only {accepted} accepted");
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
    crate::compiler::compile_method(method, &resolved.bindings, lowering)
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

/// What the analysis spent on one body, against its size.
struct Measured {
    ratio: f64,
    memory_ratio: f64,
    open_entries: usize,
    /// The peak against the fewest bytes the body can take on the wire --
    /// an instruction's tag and one register, two bytes for each register
    /// it reads -- for a body of at least `LARGE` of them.
    memory_per_byte: Option<f64>,
}

/// A body this large on the wire is measured against its bytes: below it,
/// the allowance's fixed costs dominate.
const LARGE: u64 = 1024;

fn measure(code: &CompiledCode) -> Result<Measured, String> {
    let instructions = code.instructions();
    let spent = check(&instructions, code.local_count, code.register_count)
        .map_err(|refusal| format!("{refusal:?}"))?;
    let bytes = spent.bytes;
    Ok(Measured {
        ratio: spent.work as f64 / spent.units as f64,
        memory_ratio: spent.peak_memory as f64 / spent.bytes as f64,
        open_entries: spent.peak_open_entries,
        memory_per_byte: (bytes >= LARGE).then(|| spent.peak_memory as f64 / bytes as f64),
    })
}

#[test]
fn every_golden_body_stays_within_the_bound() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    sources_under(&root.join("goldens"), &mut files);
    sources_under(&root.join("benchmarks"), &mut files);
    let mut bodies = 0usize;
    let mut modules = 0usize;
    let mut worst_ratio = (0.0f64, String::new());
    let mut worst_memory = (0.0f64, String::new());
    let mut most_open = (0usize, String::new());
    let mut worst_per_byte = (0.0f64, String::new());
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
                let label = format!("{name} {lowering:?} ({} instructions)", instructions.len());
                match measure(&code) {
                    Ok(measured) => {
                        if measured.ratio > worst_ratio.0 {
                            worst_ratio = (measured.ratio, label.clone());
                        }
                        if measured.memory_ratio > worst_memory.0 {
                            worst_memory = (measured.memory_ratio, label.clone());
                        }
                        if measured.open_entries > most_open.0 {
                            most_open = (measured.open_entries, label.clone());
                        }
                        if let Some(per_byte) = measured.memory_per_byte {
                            if per_byte > worst_per_byte.0 {
                                worst_per_byte = (per_byte, label.clone());
                            }
                        }
                        if let Ok(leftovers) =
                            analyze(&instructions, code.local_count, code.register_count)
                        {
                            if instructions.len() <= 400 {
                                if let Err(message) = agrees_with_the_oracle(
                                    &leftovers,
                                    &instructions,
                                    code.local_count,
                                    code.register_count,
                                ) {
                                    failures.push(format!("{label}: {message}"));
                                }
                            }
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
    eprintln!(
        "{modules} modules, {bodies} bodies; largest work ratio {:.2} ({}), largest memory ratio {:.2} ({}), most open entries {} ({})",
        worst_ratio.0, worst_ratio.1, worst_memory.0, worst_memory.1, most_open.0, most_open.1
    );
    eprintln!(
        "largest memory per encoded byte of a large body {:.2} ({})",
        worst_per_byte.0, worst_per_byte.1
    );
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

/// The flat worst case of the state's size: one keyword send of `width`
/// parts, each an inlined conditional over a value an inner send left a
/// tainted register beside.
fn flat(width: usize) -> String {
    let argument = "((a f: (b g: a)) ifTrue: [a] ifFalse: [b])";
    let mut expression = String::from("b");
    for _ in 0..width {
        expression.push_str(&format!(" f: {argument}"));
    }
    method_source(&format!("^{expression}"))
}

/// Compiles `source`'s method under both lowerings and holds it, and every
/// block it creates, to the bound; answers the largest work ratio and the
/// most forward jumps open at once.
fn assert_within_the_bound(source: &str, what: &str) -> (f64, usize) {
    let mut worst = (0.0f64, 0usize);
    for lowering in [Lowering::Optimized, Lowering::Reference] {
        let mut codes = Vec::new();
        collect(&Rc::new(compile_first_method(source, lowering)), &mut codes);
        let mut instructions = 0;
        let mut lowering_worst = (0.0f64, 0.0f64, 0usize);
        let mut per_byte = 0.0f64;
        for code in &codes {
            instructions += code.instruction_count();
            let measured = match measure(code) {
                Ok(measured) => measured,
                Err(refusal) => panic!("{what} {lowering:?}: {refusal}"),
            };
            assert!(measured.ratio <= WORK_PER_UNIT as f64);
            assert!(measured.memory_per_byte.unwrap_or(0.0) <= MEMORY_PER_BYTE as f64);
            assert!(measured.open_entries <= OPEN_ENTRY_LIMIT);
            per_byte = per_byte.max(measured.memory_per_byte.unwrap_or(0.0));
            lowering_worst = (
                lowering_worst.0.max(measured.ratio),
                lowering_worst.1.max(measured.memory_ratio),
                lowering_worst.2.max(measured.open_entries),
            );
        }
        eprintln!(
            "{what} {lowering:?}: {} bodies, {instructions} instructions, work ratio {:.2}, memory ratio {:.2}, memory per byte {:.2}, open entries {}",
            codes.len(),
            lowering_worst.0,
            lowering_worst.1,
            per_byte,
            lowering_worst.2
        );
        worst = (worst.0.max(lowering_worst.0), worst.1.max(lowering_worst.2));
    }
    worst
}

#[test]
fn the_flat_worst_case_stays_within_the_bound() {
    for width in [500, 2000] {
        assert_within_the_bound(&flat(width), &format!("flat {width}"));
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
        let (ratio, open) = assert_within_the_bound(
            &nested(depth, shape, innermost),
            &format!("{shape} x{depth}"),
        );
        assert!(open <= OPEN_ENTRY_LIMIT, "{shape}: {open} open, {ratio}");
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
