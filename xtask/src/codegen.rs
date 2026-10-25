//! `cargo xtask gates codegen`: holds the machine code of the functions on
//! the send, push and return path to a budget. Twice an LLVM codegen change
//! cost 10-28% on the benchmarks and was found only after hours of
//! measuring: a 16-byte `Arguments` and later a 72-byte `ActivationHeader`
//! were written onto the stack in narrow stores and read back with 16-byte
//! loads, which the processor cannot forward from the stores and so stalls
//! on, and `Mutator::push_sent` grew from 276 to 7,265 instructions when a
//! push chain was inlined into it. Both show in the listing of the
//! release build long before a measurement finds them, so this gate reads
//! the listing at commit time: for each function of `BUDGETS`, the count of
//! its instructions and the count of its wide loads over narrower stack
//! stores, each against a budget.
//!
//! The listing is `cargo asm`'s (`cargo-show-asm`) of `ns-runtime`'s lib in
//! the `release` profile, for `x86_64-pc-windows-msvc` whatever the host,
//! so that the budgets mean the same machine code everywhere; the build
//! lives in a target directory of its own, `target/codegen`, so that it
//! never invalidates the other builds' artifacts.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

/// The target the budgets were measured for: the listing is target code.
const TARGET: &str = "x86_64-pc-windows-msvc";

/// Where the build and its listing go, relative to the checked tree.
const TARGET_DIRECTORY: &str = "target/codegen";

/// The install command a missing `cargo asm` is answered with.
const INSTALL: &str = "cargo install cargo-show-asm --locked";

/// One function held to a budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Budget {
    /// The function's demangled name as the listing writes it: a free
    /// function's Rust path, a method's `<Type>::method`.
    function: &'static str,
    /// The most instructions its code may have, the cleanup funclets
    /// included.
    instructions: usize,
    /// The most 16- or 32-byte loads from the stack over an earlier,
    /// narrower store to the same bytes (`wide_loads_over_narrower_stores`).
    wide_loads: usize,
}

/// The out-of-line functions of the send, push and return path, with their
/// budgets: those the profiles of the two incidents named, and the rest of
/// the path the compiler keeps out of line (`place_arguments`, a native
/// loop's `resume`).
///
/// Each budget is the count measured at `d7488e20` with a headroom: 10% on
/// the instructions, rounded up to a multiple of ten, and 10% but at least
/// two on the wide loads. An ordinary change moves a function's size by a
/// few percent (`push_sent` grew 2% over the four commits after
/// `6d7da106`; a change to the switch sites, reverted, grew `vm::run` 1%)
/// and its wide loads by one or two, which the headroom absorbs. The
/// incidents went far past it: `push_sent` grew from 276 to 7,265
/// instructions when a push chain was inlined into it, and at `0037350d`
/// it had 6,588 instructions and 79 wide loads against 4,793 and 12 once
/// fixed. A change that passes a budget for a reason is a decision:
/// measure it (the `measuring-performance` skill), then raise the budget
/// with the reason in the same commit.
///
/// `builtins::execute`'s wide loads include the stall `docs/OPEN-WORK.md`
/// keeps open (its result comes back through memory, and `vm::run` copies
/// it out); the budgets keep their number from growing. The cold refusals
/// (`push_refused`, `push_past_the_limit`) are not on the path and are not
/// held.
const BUDGETS: &[Budget] = &[
    // Measured 8,471 instructions and 49 wide loads.
    Budget {
        function: "ns_runtime::vm::run",
        instructions: 9_320,
        wide_loads: 54,
    },
    // Measured 1,087 and 8.
    Budget {
        function: "ns_runtime::vm::run_wide",
        instructions: 1_200,
        wide_loads: 10,
    },
    // Measured 4,898 and 12.
    Budget {
        function: "<ns_runtime::interpreter::Mutator>::push_sent",
        instructions: 5_390,
        wide_loads: 14,
    },
    // Measured 3,071 and 8.
    Budget {
        function: "<ns_runtime::interpreter::Mutator>::push_step",
        instructions: 3_380,
        wide_loads: 10,
    },
    // Measured 1,020 and 0.
    Budget {
        function: "<ns_runtime::interpreter::Mutator>::push_initializer_placed",
        instructions: 1_130,
        wide_loads: 2,
    },
    // Measured 393 and 0.
    Budget {
        function: "<ns_runtime::heap::Heap>::pop_activation",
        instructions: 440,
        wide_loads: 2,
    },
    // Measured 519 and 0.
    Budget {
        function: "ns_runtime::vm::finish",
        instructions: 580,
        wide_loads: 2,
    },
    // Measured 271 and 0.
    Budget {
        function: "ns_runtime::vm::resume_caller",
        instructions: 300,
        wide_loads: 2,
    },
    // A native loop's return: measured 312 and 1.
    Budget {
        function: "<ns_runtime::interpreter::Mutator>::resume",
        instructions: 350,
        wide_loads: 3,
    },
    // Measured 55 and 0.
    Budget {
        function: "ns_runtime::vm::place_arguments",
        instructions: 70,
        wide_loads: 2,
    },
    // Measured 1,685 and 3.
    Budget {
        function: "ns_runtime::vm::unwind",
        instructions: 1_860,
        wide_loads: 5,
    },
    // Measured 157 and 0.
    Budget {
        function: "core::ptr::drop_glue::<ns_runtime::interpreter::Step>",
        instructions: 180,
        wide_loads: 2,
    },
    // Measured 16,985 and 169.
    Budget {
        function: "ns_runtime::builtins::execute",
        instructions: 18_690,
        wide_loads: 186,
    },
];

/// What the listing says about one function.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Measured {
    instructions: usize,
    wide_loads: usize,
    /// The `unique` numbers of the sections the function's code lies in:
    /// one, unless two functions share the demangled name.
    sections: HashSet<u32>,
}

/// Builds the listing of the tree at `tree` and holds every function of
/// `BUDGETS` to its budget; the error names what failed.
pub(crate) fn check(tree: &Path) -> Result<(), String> {
    let installed = Command::new(super::cargo())
        .args(["asm", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !installed {
        return Err(format!(
            "`cargo asm` is not installed: install it with `{INSTALL}`"
        ));
    }
    let target_directory = tree.join(TARGET_DIRECTORY);
    std::fs::create_dir_all(&target_directory)
        .map_err(|error| format!("create {}: {error}", target_directory.display()))?;
    let listing_path = target_directory.join("ns-runtime.s");
    let listing = std::fs::File::create(&listing_path)
        .map_err(|error| format!("create {}: {error}", listing_path.display()))?;
    let status = Command::new(super::cargo())
        .args(["asm", "--package", "ns-runtime", "--lib", "--release"])
        .args(["--target", TARGET, "--target-dir"])
        .arg(&target_directory)
        .args(["--intel", "--everything"])
        .current_dir(tree)
        .stdout(Stdio::from(listing))
        .status()
        .map_err(|error| format!("cargo asm: could not start: {error}"))?;
    if !status.success() {
        return Err(format!(
            "cargo asm ({status}); a host other than Windows needs `rustup target add {TARGET}`"
        ));
    }
    let listing = std::fs::File::open(&listing_path)
        .map_err(|error| format!("open {}: {error}", listing_path.display()))?;
    let names: Vec<&str> = BUDGETS.iter().map(|budget| budget.function).collect();
    let measured = measure(BufReader::new(listing), &names)
        .map_err(|error| format!("read {}: {error}", listing_path.display()))?;
    let failures = judge(&measured);
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("codegen: {}", failures.join("; ")))
    }
}

/// Prints every function's counts against its budget and answers what is
/// over budget, missing or ambiguous.
fn judge(measured: &HashMap<String, Measured>) -> Vec<String> {
    println!("{:>15} {:>12}  function", "instructions", "wide loads");
    let mut failures = Vec::new();
    for budget in BUDGETS {
        let Some(function) = measured.get(budget.function) else {
            println!("{:>15} {:>12}  {}", "-", "-", budget.function);
            failures.push(format!(
                "{} is not in the listing: inlined everywhere or renamed",
                budget.function
            ));
            continue;
        };
        let over_instructions = function.instructions > budget.instructions;
        let over_wide_loads = function.wide_loads > budget.wide_loads;
        let mark = |over: bool| if over { "!" } else { " " };
        println!(
            "{:>6} / {:>5}{} {:>4} / {:>4}{}  {}",
            function.instructions,
            budget.instructions,
            mark(over_instructions),
            function.wide_loads,
            budget.wide_loads,
            mark(over_wide_loads),
            budget.function
        );
        if function.sections.len() > 1 {
            failures.push(format!(
                "{} names {} functions",
                budget.function,
                function.sections.len()
            ));
        }
        if over_instructions {
            failures.push(format!(
                "{}: {} instructions, budget {}",
                budget.function, function.instructions, budget.instructions
            ));
        }
        if over_wide_loads {
            failures.push(format!(
                "{}: {} wide loads over narrower stores, budget {}",
                budget.function, function.wide_loads, budget.wide_loads
            ));
        }
    }
    failures
}

/// Measures the functions `names` in `listing`, an Intel-syntax COFF
/// listing as `cargo asm --everything` writes it. A function's code is
/// every instruction in the sections of its COMDAT symbol -- its body and
/// the cleanup funclets that follow it, which run in its frame -- so it is
/// what `cargo asm <function>` shows.
fn measure(listing: impl BufRead, names: &[&str]) -> std::io::Result<HashMap<String, Measured>> {
    let wanted: HashSet<&str> = names.iter().copied().collect();
    let mut lines: HashMap<String, Vec<String>> = HashMap::new();
    let mut sections: HashMap<String, HashSet<u32>> = HashMap::new();
    let mut current: Option<String> = None;
    for line in listing.lines() {
        let line = line?;
        if line.starts_with(".section") {
            current = comdat_of(&line).and_then(|(symbol, unique)| {
                wanted.contains(symbol).then(|| {
                    sections
                        .entry(symbol.to_string())
                        .or_default()
                        .insert(unique);
                    symbol.to_string()
                })
            });
            continue;
        }
        if let Some(function) = &current {
            if is_instruction(&line) {
                lines.entry(function.clone()).or_default().push(line);
            }
        }
    }
    Ok(lines
        .into_iter()
        .map(|(function, instructions)| {
            let measured = Measured {
                instructions: instructions.len(),
                wide_loads: wide_loads_over_narrower_stores(&instructions),
                sections: sections.remove(&function).unwrap_or_default(),
            };
            (function, measured)
        })
        .collect())
}

/// The COMDAT symbol and the `unique` number of a `.section` line, as in
/// `.section .text,"xr",one_only,<ns_runtime::heap::Heap>::pop_activation,unique,935`;
/// `None` for a section of no symbol's.
fn comdat_of(line: &str) -> Option<(&str, u32)> {
    let (_, rest) = line.split_once(",one_only,")?;
    let (symbol, unique) = rest.rsplit_once(",unique,")?;
    Some((symbol, unique.trim().parse().ok()?))
}

/// Whether a listing line is an instruction: indented, and neither a
/// directive (`.seh_pushreg`) nor a label, which start at the margin.
fn is_instruction(line: &str) -> bool {
    line.strip_prefix('\t')
        .and_then(|rest| rest.chars().next())
        .is_some_and(|first| first.is_ascii_lowercase())
}

/// One access to the stack: its base register, its offset from it and its
/// width in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StackAccess<'a> {
    base: &'a str,
    offset: i64,
    width: i64,
}

impl StackAccess<'_> {
    fn overlaps(&self, other: &StackAccess<'_>) -> bool {
        self.base == other.base
            && self.offset < other.offset + other.width
            && other.offset < self.offset + self.width
    }
}

/// The moves that store a value to memory and can write a narrow piece of
/// a stack temporary.
const STORES: [&str; 9] = [
    "mov", "movups", "movaps", "movdqu", "movdqa", "vmovups", "vmovaps", "vmovdqu", "vmovdqa",
];

/// The moves that load 16 or 32 bytes into a vector register.
const WIDE_LOADS: [&str; 8] = [
    "movups", "movaps", "movdqu", "movdqa", "vmovups", "vmovaps", "vmovdqu", "vmovdqa",
];

/// How many of `instructions`' 16- or 32-byte loads from the stack read
/// bytes an earlier instruction of the listing stored narrower -- the
/// pattern of both incidents, an aggregate built field by field and moved
/// as a whole. When the store is still in flight the load cannot be
/// forwarded from it and waits for it to retire. The count is static: it
/// follows the listing's order, not the control flow, and never forgets a
/// store, so it counts candidate sites, some on cold paths; what the budget
/// holds is that their number does not jump.
fn wide_loads_over_narrower_stores(instructions: &[String]) -> usize {
    let mut stores: Vec<StackAccess<'_>> = Vec::new();
    let mut count = 0;
    for instruction in instructions {
        let Some((mnemonic, operands)) = instruction.trim().split_once([' ', '\t']) else {
            continue;
        };
        let Some((destination, source)) = operands.trim().split_once(", ") else {
            continue;
        };
        if STORES.contains(&mnemonic) {
            if let Some(store) = stack_operand(destination) {
                stores.push(store);
                continue;
            }
        }
        if WIDE_LOADS.contains(&mnemonic)
            && (destination.starts_with("xmm") || destination.starts_with("ymm"))
        {
            if let Some(load) = stack_operand(source) {
                if load.width >= 16
                    && stores
                        .iter()
                        .any(|store| store.width < load.width && store.overlaps(&load))
                {
                    count += 1;
                }
            }
        }
    }
    count
}

/// The stack access a memory operand such as `qword ptr [rbp + 608]`
/// writes, or `None` for an operand that is no access relative to `rsp` or
/// `rbp` (a register, an immediate, an indexed or `rip`-relative address).
fn stack_operand(operand: &str) -> Option<StackAccess<'_>> {
    let (width, address) = operand.split_once(" ptr [")?;
    let width = match width {
        "byte" => 1,
        "word" => 2,
        "dword" => 4,
        "qword" => 8,
        "xmmword" => 16,
        "ymmword" => 32,
        _ => return None,
    };
    let address = address.strip_suffix(']')?;
    let (base, offset) = match address.split_once(' ') {
        None => (address, 0),
        Some((base, displacement)) => {
            let (sign, number) = displacement.split_once(' ')?;
            let number: i64 = number.parse().ok()?;
            match sign {
                "+" => (base, number),
                "-" => (base, -number),
                _ => return None,
            }
        }
    };
    matches!(base, "rsp" | "rbp").then_some(StackAccess {
        base,
        offset,
        width,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(listing: &str) -> Vec<String> {
        listing.lines().map(str::to_string).collect()
    }

    /// The header of stage 2's push: built on the stack by a qword, a byte
    /// and a dword store and more qwords, then moved with 16-byte loads, of
    /// which the ones over the narrow stores cannot be forwarded.
    #[test]
    fn a_wide_load_over_narrow_stores_is_counted() {
        let instructions = lines(
            "\tmov qword ptr [rbp + 40], 0\n\
             \tmov byte ptr [rbp + 56], 0\n\
             \tmov dword ptr [rbp + 60], r9d\n\
             \tmov qword ptr [rbp + 72], rax\n\
             \tmov qword ptr [rbp + 80], rcx\n\
             \tjbe .LBB12_7\n\
             \tmovdqu xmm0, xmmword ptr [rbp + 40]\n\
             \tmovdqu xmm1, xmmword ptr [rbp + 56]\n\
             \tmovups xmm2, xmmword ptr [rbp + 72]\n\
             \tmovups xmm3, xmmword ptr [rbp + 96]",
        );
        // `[rbp + 40]` spans the qword at 40, `[rbp + 56]` the byte and the
        // dword, `[rbp + 72]` the two qwords; `[rbp + 96]` nothing stored.
        assert_eq!(wide_loads_over_narrower_stores(&instructions), 3);
    }

    /// A 16-byte store read back by a 16-byte load forwards: not counted,
    /// nor are a load relative to another base and a load into a general
    /// register.
    #[test]
    fn a_wide_load_over_a_store_as_wide_is_not_counted() {
        let instructions = lines(
            "\tmovups xmmword ptr [rsp + 32], xmm6\n\
             \tmov qword ptr [rbp - 16], rax\n\
             \tmovups xmm0, xmmword ptr [rsp + 32]\n\
             \tmovups xmm1, xmmword ptr [rcx - 16]\n\
             \tmov rax, qword ptr [rbp - 16]",
        );
        assert_eq!(wide_loads_over_narrower_stores(&instructions), 0);
    }

    #[test]
    fn a_stack_operand_reads_its_width_base_and_offset() {
        assert_eq!(
            stack_operand("dword ptr [rbp - 60]"),
            Some(StackAccess {
                base: "rbp",
                offset: -60,
                width: 4
            })
        );
        assert_eq!(
            stack_operand("xmmword ptr [rsp]"),
            Some(StackAccess {
                base: "rsp",
                offset: 0,
                width: 16
            })
        );
        assert_eq!(stack_operand("qword ptr [rip + .Lconstant]"), None);
        assert_eq!(stack_operand("qword ptr [rbp + 8*rax + 16]"), None);
        assert_eq!(stack_operand("rax"), None);
    }

    /// A function is its sections' instructions, its funclets' included;
    /// directives, labels and other functions' code are not counted.
    #[test]
    fn the_listing_gives_each_function_its_size() {
        let listing = "\
\t.def\tns_runtime::vm::run;
.section .text,\"xr\",one_only,ns_runtime::vm::run,unique,7
\t.p2align\t4
ns_runtime::vm::run:
.seh_proc _RNvNtCs1_10ns_runtime2vm3run
\tpush rbp
\t.seh_pushreg rbp
\tmov qword ptr [rbp + 8], rcx
.LBB7_1:
\tmovups xmm0, xmmword ptr [rbp]
\tret
\t.seh_handlerdata
.section .text,\"xr\",one_only,ns_runtime::vm::run,unique,7
\t.seh_endproc
\"?dtor$3@?0?ns_runtime::vm::run@4HA\":
\tpush rbp
\tret
.section .text,\"xr\",one_only,<ns_runtime::heap::Heap>::pop_activation,unique,9
<ns_runtime::heap::Heap>::pop_activation:
\tmov eax, 1
\tret
.section .xdata,\"dr\",associative,<ns_runtime::heap::Heap>::pop_activation,unique,9
\t.long\t0
.section .text,\"xr\",one_only,ns_runtime::other,unique,10
\tnop
\tret
";
        let measured = measure(
            listing.as_bytes(),
            &[
                "ns_runtime::vm::run",
                "<ns_runtime::heap::Heap>::pop_activation",
            ],
        )
        .expect("a listing in memory reads");
        let run = &measured["ns_runtime::vm::run"];
        assert_eq!(run.instructions, 6);
        assert_eq!(run.wide_loads, 1);
        assert_eq!(run.sections, HashSet::from([7]));
        assert_eq!(
            measured["<ns_runtime::heap::Heap>::pop_activation"].instructions,
            2
        );
        assert_eq!(measured.len(), 2, "only the functions asked for");
    }

    #[test]
    fn every_budget_names_a_function_once() {
        let names: HashSet<&str> = BUDGETS.iter().map(|budget| budget.function).collect();
        assert_eq!(names.len(), BUDGETS.len());
    }
}
