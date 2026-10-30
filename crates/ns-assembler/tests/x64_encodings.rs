//! Every x64 operation, over every register and the displacements and
//! immediates at the edges of each encoding's forms, held to the text an
//! independent disassembler reads back from its bytes.

use iced_x86::{Code, Decoder, DecoderOptions, Formatter, Instruction, IntelFormatter, Mnemonic};
use ns_assembler::{
    x64, Assembled, Assembler, AssemblyError, Condition, Label, Memory, Register, Width, X64,
};

const QUADWORD_NAMES: [&str; 16] = [
    "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13",
    "r14", "r15",
];
const DOUBLEWORD_NAMES: [&str; 16] = [
    "eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi", "r8d", "r9d", "r10d", "r11d", "r12d",
    "r13d", "r14d", "r15d",
];
const WORD_NAMES: [&str; 16] = [
    "ax", "cx", "dx", "bx", "sp", "bp", "si", "di", "r8w", "r9w", "r10w", "r11w", "r12w", "r13w",
    "r14w", "r15w",
];
const BYTE_NAMES: [&str; 16] = [
    "al", "cl", "dl", "bl", "spl", "bpl", "sil", "dil", "r8b", "r9b", "r10b", "r11b", "r12b",
    "r13b", "r14b", "r15b",
];

/// The displacements at the edges of the forms: none, a byte, a doubleword.
const DISPLACEMENTS: [i32; 8] = [0, 1, -1, 127, 128, -128, -129, i32::MAX];

/// The immediates at the edges of the sign-extended byte and doubleword forms.
const IMMEDIATES: [i32; 9] = [0, 1, -1, 127, 128, -128, -129, i32::MAX, i32::MIN];

fn registers() -> impl Iterator<Item = Register> {
    (0..16).map(Register)
}

fn name(register: Register) -> &'static str {
    QUADWORD_NAMES[usize::from(register.0)]
}

/// A number as the Intel formatter writes it: nine and below in decimal,
/// the rest in upper-case hexadecimal with an `h`, and a leading zero
/// before a letter.
fn number(value: u64) -> String {
    if value <= 9 {
        return value.to_string();
    }
    let digits = format!("{value:X}");
    if digits.starts_with(|digit: char| digit.is_ascii_alphabetic()) {
        format!("0{digits}h")
    } else {
        format!("{digits}h")
    }
}

/// A 32-bit immediate sign-extended to a 64-bit operand.
fn immediate(value: i32) -> String {
    number(i64::from(value).cast_unsigned())
}

fn memory_text(memory: Memory) -> String {
    let base = name(memory.base);
    let displacement = i64::from(memory.displacement);
    match displacement.cmp(&0) {
        std::cmp::Ordering::Equal => format!("[{base}]"),
        std::cmp::Ordering::Greater => format!("[{base}+{}]", number(displacement.cast_unsigned())),
        std::cmp::Ordering::Less => format!("[{base}-{}]", number(displacement.unsigned_abs())),
    }
}

fn memories() -> impl Iterator<Item = Memory> {
    registers().flat_map(|base| {
        DISPLACEMENTS
            .into_iter()
            .map(move |displacement| Memory { base, displacement })
    })
}

fn decode(bytes: &[u8]) -> Vec<Instruction> {
    let mut decoder = Decoder::with_ip(64, bytes, 0, DecoderOptions::NONE);
    let mut instructions = Vec::new();
    while decoder.can_decode() {
        let instruction = decoder.decode();
        assert_ne!(
            instruction.code(),
            Code::INVALID,
            "{bytes:02X?} does not decode"
        );
        instructions.push(instruction);
    }
    instructions
}

fn disassemble(bytes: &[u8]) -> String {
    let mut formatter = IntelFormatter::new();
    let lines: Vec<String> = decode(bytes)
        .iter()
        .map(|instruction| {
            let mut text = String::new();
            formatter.format(instruction, &mut text);
            text
        })
        .collect();
    lines.join("\n")
}

fn assembled(build: impl FnOnce(&mut Assembler<X64>)) -> Assembled {
    let mut assembler = Assembler::<X64>::new();
    build(&mut assembler);
    match assembler.finish() {
        Ok(assembled) => assembled,
        Err(error) => panic!("assembly failed: {error}"),
    }
}

fn bytes(build: impl FnOnce(&mut Assembler<X64>)) -> Vec<u8> {
    assembled(build).bytes
}

fn text(build: impl FnOnce(&mut Assembler<X64>)) -> String {
    disassemble(&bytes(build))
}

#[test]
fn the_register_constants_are_the_machine_numbers() {
    let constants = [
        x64::RAX,
        x64::RCX,
        x64::RDX,
        x64::RBX,
        x64::RSP,
        x64::RBP,
        x64::RSI,
        x64::RDI,
        x64::R8,
        x64::R9,
        x64::R10,
        x64::R11,
        x64::R12,
        x64::R13,
        x64::R14,
        x64::R15,
    ];
    for (number, constant) in constants.into_iter().enumerate() {
        assert_eq!(usize::from(constant.0), number);
    }
}

#[test]
fn move_register() {
    for target in registers() {
        for source in registers() {
            assert_eq!(
                text(|assembler| assembler.move_register(target, source)),
                format!("mov {},{}", name(target), name(source))
            );
        }
    }
}

#[test]
fn move_immediate_takes_the_shortest_form() {
    let values: [u64; 9] = [
        0,
        1,
        0x7FFF_FFFF,
        0x8000_0000,
        0xFFFF_FFFF,
        0x1_0000_0000,
        u64::MAX,
        i64::from(i32::MIN).cast_unsigned(),
        0x1234_5678_9ABC_DEF0,
    ];
    for target in registers() {
        let extension = usize::from(target.0 >= 8);
        for value in values {
            let code = bytes(|assembler| assembler.move_immediate(target, value));
            let (expected, length) = if u32::try_from(value).is_ok() {
                // A 32-bit move clears the upper half.
                (
                    format!(
                        "mov {},{}",
                        DOUBLEWORD_NAMES[usize::from(target.0)],
                        number(value)
                    ),
                    5 + extension,
                )
            } else if i32::try_from(value.cast_signed()).is_ok() {
                (format!("mov {},{}", name(target), number(value)), 7)
            } else {
                (format!("mov {},{}", name(target), number(value)), 10)
            };
            assert_eq!(disassemble(&code), expected);
            assert_eq!(code.len(), length, "{expected}");
        }
    }
}

#[test]
fn load_of_every_width() {
    for target in registers() {
        for memory in memories() {
            let operand = memory_text(memory);
            let index = usize::from(target.0);
            let cases = [
                (
                    Width::Byte,
                    format!("movzx {},byte ptr {operand}", DOUBLEWORD_NAMES[index]),
                ),
                (
                    Width::Half,
                    format!("movzx {},word ptr {operand}", DOUBLEWORD_NAMES[index]),
                ),
                (
                    Width::Word32,
                    format!("mov {},{operand}", DOUBLEWORD_NAMES[index]),
                ),
                (
                    Width::Word64,
                    format!("mov {},{operand}", QUADWORD_NAMES[index]),
                ),
            ];
            for (width, expected) in cases {
                assert_eq!(
                    text(|assembler| assembler.load(width, target, memory)),
                    expected
                );
            }
        }
    }
}

#[test]
fn store_of_every_width() {
    for source in registers() {
        for memory in memories() {
            let operand = memory_text(memory);
            let index = usize::from(source.0);
            let cases = [
                (Width::Byte, format!("mov {operand},{}", BYTE_NAMES[index])),
                (Width::Half, format!("mov {operand},{}", WORD_NAMES[index])),
                (
                    Width::Word32,
                    format!("mov {operand},{}", DOUBLEWORD_NAMES[index]),
                ),
                (
                    Width::Word64,
                    format!("mov {operand},{}", QUADWORD_NAMES[index]),
                ),
            ];
            for (width, expected) in cases {
                assert_eq!(
                    text(|assembler| assembler.store(width, memory, source)),
                    expected
                );
            }
        }
    }
}

#[test]
fn store_immediate32_writes_a_sign_extended_word() {
    for memory in memories() {
        for value in IMMEDIATES {
            assert_eq!(
                text(|assembler| assembler.store_immediate32(memory, value)),
                format!("mov qword ptr {},{}", memory_text(memory), immediate(value))
            );
        }
    }
}

#[test]
fn register_arithmetic() {
    for target in registers() {
        for source in registers() {
            let (left, right) = (name(target), name(source));
            assert_eq!(
                text(|assembler| assembler.add(target, source)),
                format!("add {left},{right}")
            );
            assert_eq!(
                text(|assembler| assembler.subtract(target, source)),
                format!("sub {left},{right}")
            );
            assert_eq!(
                text(|assembler| assembler.multiply_checked(target, source)),
                format!("imul {left},{right}")
            );
            assert_eq!(
                text(|assembler| assembler.and(target, source)),
                format!("and {left},{right}")
            );
            assert_eq!(
                text(|assembler| assembler.compare(target, source)),
                format!("cmp {left},{right}")
            );
        }
    }
}

#[test]
fn immediate_operations() {
    for target in registers() {
        let register = name(target);
        for value in IMMEDIATES {
            let operand = immediate(value);
            assert_eq!(
                text(|assembler| assembler.and_immediate(target, value)),
                format!("and {register},{operand}")
            );
            assert_eq!(
                text(|assembler| assembler.or_immediate(target, value)),
                format!("or {register},{operand}")
            );
            assert_eq!(
                text(|assembler| assembler.add_immediate(target, value)),
                format!("add {register},{operand}")
            );
            assert_eq!(
                text(|assembler| assembler.compare_immediate(target, value)),
                format!("cmp {register},{operand}")
            );
            assert_eq!(
                text(|assembler| assembler.test_immediate(target, value)),
                format!("test {register},{operand}")
            );
            assert_eq!(
                text(|assembler| assembler.test_immediate_with_width(Width::Word32, target, value)),
                format!(
                    "test {},{}",
                    DOUBLEWORD_NAMES[usize::from(target.0)],
                    number(u64::from(value.cast_unsigned()))
                )
            );
        }
    }
}

#[test]
fn an_immediate_that_fits_a_byte_takes_the_short_form() {
    let length = |value: i32| bytes(|assembler| assembler.and_immediate(x64::RCX, value)).len();
    assert_eq!(length(127), 4);
    assert_eq!(length(-128), 4);
    assert_eq!(length(128), 7);
    assert_eq!(length(-129), 7);
}

#[test]
fn shifts() {
    for target in registers() {
        for count in [1_u8, 3, 63] {
            assert_eq!(
                text(|assembler| assembler.shift_left(target, count)),
                format!("shl {},{}", name(target), number(u64::from(count)))
            );
            assert_eq!(
                text(|assembler| assembler.shift_right_arithmetic(target, count)),
                format!("sar {},{}", name(target), number(u64::from(count)))
            );
            assert_eq!(
                text(|assembler| assembler.shift_right_logical(target, count)),
                format!("shr {},{}", name(target), number(u64::from(count)))
            );
        }
    }
}

#[test]
fn compare_memory() {
    for left in registers() {
        for memory in memories() {
            assert_eq!(
                text(|assembler| assembler.compare_memory(left, memory)),
                format!("cmp {},{}", name(left), memory_text(memory))
            );
        }
    }
}

#[test]
fn calls_and_the_stack() {
    for register in registers() {
        assert_eq!(
            text(|assembler| assembler.call_register(register)),
            format!("call {}", name(register))
        );
        assert_eq!(
            text(|assembler| assembler.jump_register(register)),
            format!("jmp {}", name(register))
        );
        assert_eq!(
            text(|assembler| assembler.push(register)),
            format!("push {}", name(register))
        );
        assert_eq!(
            text(|assembler| assembler.pop(register)),
            format!("pop {}", name(register))
        );
    }
    for amount in DISPLACEMENTS.into_iter().chain([i32::MIN]) {
        assert_eq!(
            text(|assembler| assembler.add_stack(amount)),
            format!(
                "lea rsp,{}",
                memory_text(Memory {
                    base: x64::RSP,
                    displacement: amount
                })
            )
        );
    }
    for register in registers() {
        // Seven bytes from address 0: the next instruction is at 7.
        assert_eq!(
            text(|assembler| assembler.load_code_address(register)),
            format!("lea {},[7]", name(register))
        );
    }
    assert_eq!(text(Assembler::return_), "ret");
    assert_eq!(text(Assembler::trap), "ud2");
}

const CONDITIONS: [(Condition, Mnemonic); 12] = [
    (Condition::Equal, Mnemonic::Je),
    (Condition::NotEqual, Mnemonic::Jne),
    (Condition::Less, Mnemonic::Jl),
    (Condition::LessOrEqual, Mnemonic::Jle),
    (Condition::Greater, Mnemonic::Jg),
    (Condition::GreaterOrEqual, Mnemonic::Jge),
    (Condition::Below, Mnemonic::Jb),
    (Condition::BelowOrEqual, Mnemonic::Jbe),
    (Condition::Above, Mnemonic::Ja),
    (Condition::AboveOrEqual, Mnemonic::Jae),
    (Condition::Overflow, Mnemonic::Jo),
    (Condition::NoOverflow, Mnemonic::Jno),
];

/// The last instruction of `bytes`: a branch, its length and its target.
fn last_branch(bytes: &[u8]) -> (Mnemonic, usize, u64) {
    let instructions = decode(bytes);
    let Some(branch) = instructions.last() else {
        panic!("no instruction");
    };
    (branch.mnemonic(), branch.len(), branch.near_branch_target())
}

#[test]
fn a_backward_branch_within_reach_is_short() {
    let code = bytes(|assembler| {
        let label = assembler.label();
        assembler.bind(label);
        assembler.return_();
        assembler.trap();
        assembler.jump(label);
    });
    assert_eq!(code.len(), 5);
    assert_eq!(last_branch(&code), (Mnemonic::Jmp, 2, 0));
    for (condition, mnemonic) in CONDITIONS {
        let code = bytes(|assembler| {
            let label = assembler.label();
            assembler.bind(label);
            assembler.return_();
            assembler.trap();
            assembler.jump_if(condition, label);
        });
        assert_eq!(last_branch(&code), (mnemonic, 2, 0));
    }
}

#[test]
fn a_backward_branch_reaches_short_exactly_as_far_as_a_byte_does() {
    // Sixty-three traps put the branch's end 128 bytes past the label, the
    // farthest a signed byte reaches; sixty-four put it 130 bytes past.
    for (traps, length) in [(63, 2), (64, 5)] {
        let code = bytes(|assembler| {
            let label = assembler.label();
            assembler.bind(label);
            for _ in 0..traps {
                assembler.trap();
            }
            assembler.jump(label);
        });
        assert_eq!(last_branch(&code), (Mnemonic::Jmp, length, 0));
    }
    for (traps, length) in [(63, 2), (64, 6)] {
        let code = bytes(|assembler| {
            let label = assembler.label();
            assembler.bind(label);
            for _ in 0..traps {
                assembler.trap();
            }
            assembler.jump_if(Condition::Overflow, label);
        });
        assert_eq!(last_branch(&code), (Mnemonic::Jo, length, 0));
    }
}

#[test]
fn a_forward_branch_is_long() {
    let code = bytes(|assembler| {
        let label = assembler.label();
        assembler.jump(label);
        assembler.trap();
        assembler.bind(label);
        assembler.return_();
    });
    assert_eq!(disassemble(&code).lines().count(), 3);
    assert_eq!(decode(&code)[0].len(), 5);
    assert_eq!(decode(&code)[0].near_branch_target(), 7);
    for (condition, mnemonic) in CONDITIONS {
        let code = bytes(|assembler| {
            let label = assembler.label();
            assembler.jump_if(condition, label);
            assembler.trap();
            assembler.bind(label);
            assembler.return_();
        });
        let first = decode(&code)[0];
        assert_eq!(
            (first.mnemonic(), first.len(), first.near_branch_target()),
            (mnemonic, 6, 8)
        );
    }
}

#[test]
fn branches_in_both_directions_land_on_their_labels() {
    let result = assembled(|assembler| {
        let top = assembler.label();
        let bottom = assembler.label();
        // A label never bound and never branched to is no error.
        assembler.label();
        assembler.bind(top);
        assembler.compare_immediate(x64::RAX, 0);
        assembler.jump_if(Condition::Equal, bottom);
        assembler.add(x64::RAX, x64::RCX);
        assembler.jump(top);
        assembler.bind(bottom);
        assembler.return_();
    });
    let instructions = decode(&result.bytes);
    let offsets: Vec<u64> = instructions.iter().map(Instruction::ip).collect();
    assert_eq!(instructions[1].near_branch_target(), offsets[4]);
    assert_eq!(instructions[3].near_branch_target(), 0);
    let bottom = u32::try_from(offsets[4]).unwrap();
    assert_eq!(result.label_offsets, vec![Some(0), Some(bottom), None]);
}

#[test]
fn an_unbound_label_is_an_error() {
    let mut assembler = Assembler::<X64>::new();
    let bound = assembler.label();
    let unbound = assembler.label();
    assembler.bind(bound);
    assembler.jump(unbound);
    assert!(matches!(
        assembler.finish(),
        Err(AssemblyError::UnboundLabel(label)) if label == unbound
    ));
}

#[test]
#[should_panic(expected = "bound twice")]
fn a_label_bound_twice_is_refused() {
    let mut assembler = Assembler::<X64>::new();
    let label: Label = assembler.label();
    assembler.bind(label);
    assembler.bind(label);
}

#[test]
fn a_forward_branch_is_patched_past_a_byte_s_reach() {
    let code = bytes(|assembler| {
        let label = assembler.label();
        assembler.jump_if(Condition::NotEqual, label);
        for _ in 0..1000 {
            assembler.trap();
        }
        assembler.bind(label);
        assembler.return_();
    });
    let first = decode(&code)[0];
    assert_eq!(
        (first.mnemonic(), first.len(), first.near_branch_target()),
        (Mnemonic::Jne, 6, 2006)
    );
}

#[test]
fn an_assembler_over_a_configured_encoder() {
    let mut assembler = Assembler::with_encoder(X64);
    assembler.return_();
    let Ok(assembled) = assembler.finish() else {
        panic!("assembly failed");
    };
    assert_eq!(disassemble(&assembled.bytes), "ret");
}

/// The conditional move's mnemonic suffix of each condition, as the
/// Intel formatter writes it.
const MOVE_SUFFIXES: [(Condition, &str); 12] = [
    (Condition::Equal, "e"),
    (Condition::NotEqual, "ne"),
    (Condition::Less, "l"),
    (Condition::LessOrEqual, "le"),
    (Condition::Greater, "g"),
    (Condition::GreaterOrEqual, "ge"),
    (Condition::Below, "b"),
    (Condition::BelowOrEqual, "be"),
    (Condition::Above, "a"),
    (Condition::AboveOrEqual, "ae"),
    (Condition::Overflow, "o"),
    (Condition::NoOverflow, "no"),
];

#[test]
fn a_conditional_move_of_every_condition() {
    for (condition, suffix) in MOVE_SUFFIXES {
        for target in registers() {
            for source in registers() {
                assert_eq!(
                    text(|assembler| assembler.move_if(condition, target, source)),
                    format!("cmov{suffix} {},{}", name(target), name(source))
                );
            }
        }
    }
}

#[test]
fn a_signed_division_sign_extends_rax_then_divides() {
    for divisor in registers().filter(|register| *register != x64::RAX && *register != x64::RDX) {
        assert_eq!(
            text(|assembler| assembler.divide_signed(x64::RAX, x64::RDX, divisor)),
            format!(
                "cqo
idiv {}",
                name(divisor)
            )
        );
    }
}

#[test]
#[should_panic(expected = "x64 divides rax")]
fn a_division_of_another_register_is_refused() {
    let _ = bytes(|assembler| assembler.divide_signed(x64::RCX, x64::RDX, x64::RSI));
}

#[test]
#[should_panic(expected = "divisor cannot be rax or rdx")]
fn a_division_by_rdx_is_refused() {
    let _ = bytes(|assembler| assembler.divide_signed(x64::RAX, x64::RDX, x64::RDX));
}
