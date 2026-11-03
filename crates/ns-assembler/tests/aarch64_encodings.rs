//! Every AArch64 operation of moves, memory, arithmetic, logic and
//! compares, over every operand register and the displacements and
//! immediates at the edges of each encoding's forms, held to the text an
//! independent disassembler, `yaxpeax-arm`, reads back from its bytes.
//!
//! The expected text is `yaxpeax-arm`'s: the preferred alias (`mov`, `cmp`,
//! `cmn`, `tst`, `lsl`, `lsr`, `asr`), immediates in lower-case hexadecimal
//! behind `#0x`, a negative offset as `#-0x…`, a move-wide immediate as the
//! value the register receives, and one line per instruction.
//! `the_disassembler_writes_the_text_these_tests_expect` pins that format
//! against instruction words assembled by hand from the Arm manual.

use ns_assembler::{aarch64, Aarch64, Assembler, Condition, Encoder, Memory, Register, Width};
use yaxpeax_arch::{Decoder, U8Reader};
use yaxpeax_arm::armv8::a64::{InstDecoder, Instruction};

/// The bytes of one instruction.
const INSTRUCTION_BYTES: usize = 4;

/// The registers the encoder keeps for itself: x16 and x17, its scratch
/// registers, x18, the platform register, and x30, the link register.
const RESERVED: [u8; 4] = [16, 17, 18, 30];

/// The displacements at the edges of the forms: the scaled unsigned
/// `imm12` (to 4095 elements), the unscaled signed `imm9` (-256 to 255)
/// and the register offset beyond both.
const DISPLACEMENTS: [i32; 13] = [
    0,
    1,
    8,
    -8,
    255,
    -256,
    256,
    -257,
    4095,
    4096,
    32760,
    32768,
    i32::MAX,
];

/// The immediates at the edges of the `imm12` form, its `lsl #12` form and
/// the materialization through a scratch register.
const IMMEDIATES: [i32; 14] = [
    0,
    1,
    -1,
    4095,
    -4095,
    4096,
    -4096,
    4097,
    0x1000 << 12,
    0xFFF << 12,
    -(0xFFF << 12),
    0x1001 << 12,
    i32::MIN,
    i32::MAX,
];

/// Logical immediates: bitmask patterns, and values that are none.
const LOGICAL_IMMEDIATES: [i32; 12] = [
    0,
    1,
    -1,
    -16,
    0xFF,
    0x5555_5555,
    0x0F0F_0F0F,
    0x7FFF_FFFF,
    i32::MIN,
    0x1234,
    3,
    0x7,
];

/// 64-bit constants with one, two, three and four non-zero halfwords, and
/// their complements.
const WIDE_VALUES: [u64; 10] = [
    0,
    0x10,
    0x1_0000,
    0xFFFF_0000_0000_0000,
    0x1234_0000_5678,
    0x1234_5678_9ABC,
    0x1234_5678_9ABC_DEF0,
    0x8000_0000_0000_0001,
    0x0000_0001_0000_0001,
    0x7FFF_FFFF,
];

fn allowed() -> impl Iterator<Item = Register> + Clone {
    (0..=30)
        .filter(|number| !RESERVED.contains(number))
        .map(Register)
}

fn allowed_or_stack() -> impl Iterator<Item = Register> + Clone {
    allowed().chain([aarch64::SP])
}

fn name(register: Register) -> String {
    if register == aarch64::SP {
        "sp".to_owned()
    } else {
        format!("x{}", register.0)
    }
}

fn word_name(register: Register) -> String {
    format!("w{}", register.0)
}

fn hexadecimal(value: u64) -> String {
    format!("#0x{value:x}")
}

fn signed_hexadecimal(value: i64) -> String {
    if value < 0 {
        format!("#-0x{:x}", value.unsigned_abs())
    } else {
        format!("#0x{value:x}")
    }
}

fn sign_extended(value: i32) -> u64 {
    i64::from(value).cast_unsigned()
}

fn disassemble(bytes: &[u8]) -> String {
    assert_eq!(bytes.len() % INSTRUCTION_BYTES, 0, "{bytes:02X?}");
    let decoder = InstDecoder::default();
    let lines: Vec<String> = bytes
        .chunks(INSTRUCTION_BYTES)
        .map(|word| {
            let mut instruction = Instruction::default();
            if decoder
                .decode_into(&mut instruction, &mut U8Reader::new(word))
                .is_err()
            {
                panic!("{word:02X?} does not decode");
            }
            instruction.to_string()
        })
        .collect();
    lines.join("\n")
}

fn bytes(build: impl FnOnce(&mut Assembler<Aarch64>)) -> Vec<u8> {
    let mut assembler = Assembler::<Aarch64>::new();
    build(&mut assembler);
    match assembler.finish() {
        Ok(assembled) => assembled.bytes,
        Err(error) => panic!("assembly failed: {error}"),
    }
}

fn text(build: impl FnOnce(&mut Assembler<Aarch64>)) -> String {
    disassemble(&bytes(build))
}

/// The instructions that put `value` into `register`: the shorter of a
/// `movz` and a `movn` sequence, `movz` on a tie, lowest halfword first.
fn move_lines(register: &str, value: u64) -> Vec<String> {
    let halfwords: Vec<u64> = (0..4)
        .map(|index| (value >> (16 * index)) & 0xFFFF)
        .collect();
    let non_zero = halfwords.iter().filter(|halfword| **halfword != 0).count();
    let non_ones = halfwords
        .iter()
        .filter(|halfword| **halfword != 0xFFFF)
        .count();
    let (inverted, skipped) = if non_ones < non_zero {
        (true, 0xFFFF)
    } else {
        (false, 0)
    };
    let first = halfwords
        .iter()
        .position(|halfword| *halfword != skipped)
        .unwrap_or(0);
    let first_value = if inverted {
        !((!halfwords[first] & 0xFFFF) << (16 * first))
    } else {
        halfwords[first] << (16 * first)
    };
    let mut lines = vec![format!("mov {register}, {}", hexadecimal(first_value))];
    for (index, halfword) in halfwords.iter().enumerate().skip(first + 1) {
        if *halfword != skipped {
            let shift = 16 * index;
            lines.push(format!(
                "movk {register}, {}, lsl #{shift}",
                hexadecimal(*halfword)
            ));
        }
    }
    lines
}

/// A width's log2 of bytes, and its load and store mnemonics: scaled,
/// unscaled.
fn mnemonics(width: Width) -> (i64, [&'static str; 4]) {
    match width {
        Width::Byte => (0, ["ldrb", "ldurb", "strb", "sturb"]),
        Width::Half => (1, ["ldrh", "ldurh", "strh", "sturh"]),
        Width::Word32 => (2, ["ldr", "ldur", "str", "stur"]),
        Width::Word64 => (3, ["ldr", "ldur", "str", "stur"]),
    }
}

fn data_name(width: Width, register: &str) -> String {
    if width == Width::Word64 {
        register.to_owned()
    } else {
        register.replacen('x', "w", 1)
    }
}

/// The instructions of a load (or a store) of `register` at `memory`.
fn access_lines(width: Width, load: bool, register: &str, memory: Memory) -> Vec<String> {
    let (size, names) = mnemonics(width);
    let (scaled, unscaled) = if load {
        (names[0], names[1])
    } else {
        (names[2], names[3])
    };
    let data = data_name(width, register);
    let base = name(memory.base);
    let displacement = i64::from(memory.displacement);
    let scale = 1_i64 << size;
    if displacement >= 0 && displacement % scale == 0 && displacement / scale <= 4095 {
        if displacement == 0 {
            vec![format!("{scaled} {data}, [{base}]")]
        } else {
            vec![format!(
                "{scaled} {data}, [{base}, {}]",
                hexadecimal(displacement.cast_unsigned())
            )]
        }
    } else if (-256..=255).contains(&displacement) {
        vec![format!(
            "{unscaled} {data}, [{base}, {}]",
            signed_hexadecimal(displacement)
        )]
    } else {
        let mut lines = move_lines("x16", sign_extended(memory.displacement));
        lines.push(format!("{scaled} {data}, [{base}, x16]"));
        lines
    }
}

/// An immediate's `imm12` operand text, its own or shifted, of its
/// magnitude, when it has one.
fn arithmetic_operand(magnitude: u64) -> Option<String> {
    if magnitude <= 0xFFF {
        Some(hexadecimal(magnitude))
    } else if magnitude & 0xFFF == 0 && magnitude >> 12 <= 0xFFF {
        Some(format!("{}, lsl #12", hexadecimal(magnitude >> 12)))
    } else {
        None
    }
}

fn joined(lines: &[String]) -> String {
    lines.join("\n")
}

fn memories(bases: impl Iterator<Item = Register> + Clone) -> Vec<Memory> {
    bases
        .flat_map(|base| {
            DISPLACEMENTS
                .into_iter()
                .map(move |displacement| Memory { base, displacement })
        })
        .collect()
}

#[test]
fn the_disassembler_writes_the_text_these_tests_expect() {
    // Words assembled by hand from the Arm manual's encodings.
    let pinned: [(u32, &str); 12] = [
        (0xAA01_03E0, "mov x0, x1"),
        (0x9100_03E0, "mov x0, sp"),
        (0xD2A0_0020, "mov x0, #0x10000"),
        (0x92A0_0020, "mov x0, #0xfffffffffffeffff"),
        (0xF2A0_0020, "movk x0, #0x1, lsl #16"),
        (0xF85F_8020, "ldur x0, [x1, #-0x8]"),
        (0xF940_0420, "ldr x0, [x1, #0x8]"),
        (0xF870_6820, "ldr x0, [x1, x16]"),
        (0xB140_041F, "cmn x0, #0x1, lsl #12"),
        (0xEB30_63FF, "cmp sp, x16"),
        (0x9240_021F, "and sp, x16, #0x1"),
        (0xD37F_F800, "lsl x0, x0, #0x1"),
    ];
    for (word, expected) in pinned {
        assert_eq!(disassemble(&word.to_le_bytes()), expected);
    }
}

#[test]
fn the_register_constants_are_the_machine_numbers() {
    let constants = [
        aarch64::X0,
        aarch64::X1,
        aarch64::X2,
        aarch64::X3,
        aarch64::X4,
        aarch64::X5,
        aarch64::X6,
        aarch64::X7,
        aarch64::X8,
        aarch64::X9,
        aarch64::X10,
        aarch64::X11,
        aarch64::X12,
        aarch64::X13,
        aarch64::X14,
        aarch64::X15,
        aarch64::X16,
        aarch64::X17,
        aarch64::X18,
        aarch64::X19,
        aarch64::X20,
        aarch64::X21,
        aarch64::X22,
        aarch64::X23,
        aarch64::X24,
        aarch64::X25,
        aarch64::X26,
        aarch64::X27,
        aarch64::X28,
        aarch64::X29,
        aarch64::X30,
        aarch64::SP,
    ];
    for (number, constant) in constants.into_iter().enumerate() {
        assert_eq!(usize::from(constant.0), number);
    }
}

#[test]
fn aarch64_has_sixteen_byte_slots_and_four_byte_instructions() {
    assert_eq!(Aarch64.stack_slot_bytes(), 16);
    assert_eq!(Aarch64.instruction_alignment(), 4);
    let mut assembler = Assembler::<Aarch64>::new();
    assembler.move_register(aarch64::X0, aarch64::X1);
    match assembler.finish() {
        Ok(assembled) => assert_eq!(assembled.alignment, 4),
        Err(error) => panic!("assembly failed: {error}"),
    }
}

#[test]
fn a_move_between_registers_is_an_orr_or_through_the_stack_pointer_an_add() {
    for target in allowed_or_stack() {
        for source in allowed_or_stack() {
            assert_eq!(
                text(|assembler| assembler.move_register(target, source)),
                format!("mov {}, {}", name(target), name(source))
            );
            let word = bytes(|assembler| assembler.move_register(target, source));
            let stack_pointer = target == aarch64::SP || source == aarch64::SP;
            // `orr` (shifted register) has opcode bits 0xAA, `add`
            // (immediate) 0x91.
            assert_eq!(word[3], if stack_pointer { 0x91 } else { 0xAA });
        }
    }
}

#[test]
fn a_move_of_an_immediate_takes_the_shorter_of_movz_and_movn() {
    let values = WIDE_VALUES
        .into_iter()
        .flat_map(|value| [value, !value])
        .chain(IMMEDIATES.into_iter().map(sign_extended));
    for target in allowed() {
        for value in values.clone() {
            let lines = move_lines(&name(target), value);
            assert!(lines.len() <= 4);
            assert_eq!(
                text(|assembler| assembler.move_immediate(target, value)),
                joined(&lines),
                "{value:#x}"
            );
        }
    }
}

#[test]
fn a_conditional_move_is_a_csel_keeping_the_target() {
    for target in allowed() {
        for source in allowed() {
            assert_eq!(
                text(|assembler| assembler.move_if(Condition::Equal, target, source)),
                format!("csel {0}, {1}, {0}, eq", name(target), name(source))
            );
        }
    }
}

#[test]
fn every_condition_has_its_code() {
    let conditions = [
        (Condition::Equal, "eq"),
        (Condition::NotEqual, "ne"),
        (Condition::Less, "lt"),
        (Condition::LessOrEqual, "le"),
        (Condition::Greater, "gt"),
        (Condition::GreaterOrEqual, "ge"),
        (Condition::Below, "lo"),
        (Condition::BelowOrEqual, "ls"),
        (Condition::Above, "hi"),
        (Condition::AboveOrEqual, "hs"),
        (Condition::Overflow, "vs"),
        (Condition::NoOverflow, "vc"),
    ];
    for (condition, code) in conditions {
        assert_eq!(
            text(|assembler| assembler.move_if(condition, aarch64::X3, aarch64::X5)),
            format!("csel x3, x5, x3, {code}")
        );
    }
}

#[test]
fn a_load_takes_the_scaled_form_when_it_fits() {
    for width in [Width::Byte, Width::Half, Width::Word32, Width::Word64] {
        for target in allowed() {
            for memory in memories(allowed_or_stack()) {
                assert_eq!(
                    text(|assembler| assembler.load(width, target, memory)),
                    joined(&access_lines(width, true, &name(target), memory)),
                    "{width:?} {memory:?}"
                );
            }
        }
    }
}

#[test]
fn a_load_within_imm9_takes_the_unscaled_form() {
    let memory = Memory {
        base: aarch64::X1,
        displacement: -8,
    };
    assert_eq!(
        text(|assembler| assembler.load(Width::Word64, aarch64::X0, memory)),
        "ldur x0, [x1, #-0x8]"
    );
    let memory = Memory {
        base: aarch64::SP,
        displacement: 3,
    };
    assert_eq!(
        text(|assembler| assembler.load(Width::Half, aarch64::X0, memory)),
        "ldurh w0, [sp, #0x3]"
    );
}

#[test]
fn a_load_beyond_imm9_goes_through_x16() {
    let memory = Memory {
        base: aarch64::X2,
        displacement: 32768,
    };
    assert_eq!(
        text(|assembler| assembler.load(Width::Word64, aarch64::X0, memory)),
        "mov x16, #0x8000\nldr x0, [x2, x16]"
    );
    let memory = Memory {
        base: aarch64::X2,
        displacement: -257,
    };
    assert_eq!(
        text(|assembler| assembler.load(Width::Byte, aarch64::X0, memory)),
        "mov x16, #0xfffffffffffffeff\nldrb w0, [x2, x16]"
    );
}

#[test]
fn a_store_takes_the_same_forms_as_a_load() {
    for width in [Width::Byte, Width::Half, Width::Word32, Width::Word64] {
        for source in allowed() {
            for memory in memories(allowed_or_stack()) {
                assert_eq!(
                    text(|assembler| assembler.store(width, memory, source)),
                    joined(&access_lines(width, false, &name(source), memory)),
                    "{width:?} {memory:?}"
                );
            }
        }
    }
}

#[test]
fn a_store_of_an_immediate_stores_xzr_for_zero_and_x17_otherwise() {
    for memory in memories(allowed_or_stack()) {
        for value in IMMEDIATES {
            let expected = if value == 0 {
                access_lines(Width::Word64, false, "xzr", memory)
            } else {
                let mut lines = move_lines("x17", sign_extended(value));
                lines.extend(access_lines(Width::Word64, false, "x17", memory));
                lines
            };
            assert_eq!(
                text(|assembler| assembler.store_immediate32(memory, value)),
                joined(&expected),
                "{memory:?} {value}"
            );
        }
    }
}

#[test]
fn an_add_and_a_subtract_set_the_flags() {
    for target in allowed() {
        for source in allowed() {
            let (left, right) = (name(target), name(source));
            assert_eq!(
                text(|assembler| assembler.add(target, source)),
                format!("adds {left}, {left}, {right}")
            );
            assert_eq!(
                text(|assembler| assembler.subtract(target, source)),
                format!("subs {left}, {left}, {right}")
            );
        }
    }
}

#[test]
fn an_add_of_an_immediate_takes_imm12_its_shifted_form_or_x16() {
    for target in allowed() {
        let register = name(target);
        for value in IMMEDIATES {
            let magnitude = i64::from(value).unsigned_abs();
            let mnemonic = if value < 0 { "subs" } else { "adds" };
            let expected = match arithmetic_operand(magnitude) {
                Some(operand) => vec![format!("{mnemonic} {register}, {register}, {operand}")],
                None => {
                    let mut lines = move_lines("x16", sign_extended(value));
                    lines.push(format!("adds {register}, {register}, x16"));
                    lines
                }
            };
            assert_eq!(
                text(|assembler| assembler.add_immediate(target, value)),
                joined(&expected),
                "{value}"
            );
        }
    }
}

#[test]
fn an_add_to_the_stack_pointer_keeps_the_flags() {
    for amount in IMMEDIATES.into_iter().chain([16, -16]) {
        let magnitude = i64::from(amount).unsigned_abs();
        let mnemonic = if amount < 0 { "sub" } else { "add" };
        let expected = match arithmetic_operand(magnitude) {
            // `add sp, sp, #0` is the `mov` alias.
            Some(_) if amount == 0 => vec!["mov sp, sp".to_owned()],
            Some(operand) => vec![format!("{mnemonic} sp, sp, {operand}")],
            None => {
                let mut lines = move_lines("x16", sign_extended(amount));
                lines.push("add sp, sp, x16".to_owned());
                lines
            }
        };
        assert_eq!(
            text(|assembler| assembler.add_stack(amount)),
            joined(&expected),
            "{amount}"
        );
    }
}

#[test]
fn an_and_of_registers_is_an_and() {
    for target in allowed() {
        for source in allowed() {
            assert_eq!(
                text(|assembler| assembler.and(target, source)),
                format!("and {0}, {0}, {1}", name(target), name(source))
            );
        }
    }
}

/// The instructions of `mnemonic` (`and`, `orr`) of `value` into
/// `target`.
fn logical_lines(mnemonic: &str, target: Register, value: i32) -> Vec<String> {
    let encodable = aarch64::logical_immediate(sign_extended(value)).is_some();
    let immediate = hexadecimal(sign_extended(value));
    if target == aarch64::SP {
        let mut lines = vec!["mov x16, sp".to_owned()];
        if encodable {
            lines.push(format!("{mnemonic} sp, x16, {immediate}"));
        } else {
            lines.extend(move_lines("x17", sign_extended(value)));
            lines.push(format!("{mnemonic} x16, x16, x17"));
            lines.push("mov sp, x16".to_owned());
        }
        lines
    } else {
        let register = name(target);
        if encodable {
            vec![format!("{mnemonic} {register}, {register}, {immediate}")]
        } else {
            let mut lines = move_lines("x16", sign_extended(value));
            lines.push(format!("{mnemonic} {register}, {register}, x16"));
            lines
        }
    }
}

#[test]
fn an_and_and_an_or_of_an_immediate_take_the_bitmask_or_x16() {
    for target in allowed_or_stack() {
        for value in LOGICAL_IMMEDIATES.into_iter().chain(IMMEDIATES) {
            assert_eq!(
                text(|assembler| assembler.and_immediate(target, value)),
                joined(&logical_lines("and", target, value)),
                "{value:#x}"
            );
            assert_eq!(
                text(|assembler| assembler.or_immediate(target, value)),
                joined(&logical_lines("orr", target, value)),
                "{value:#x}"
            );
        }
    }
}

#[test]
fn an_and_into_the_stack_pointer_goes_through_x16() {
    assert_eq!(
        text(|assembler| assembler.and_immediate(aarch64::SP, -16)),
        "mov x16, sp\nand sp, x16, #0xfffffffffffffff0"
    );
}

#[test]
fn shifts_are_the_bitfield_move_aliases() {
    for target in allowed() {
        let register = name(target);
        for count in [1_u8, 3, 32, 63] {
            let amount = hexadecimal(u64::from(count));
            assert_eq!(
                text(|assembler| assembler.shift_left(target, count)),
                format!("lsl {register}, {register}, {amount}")
            );
            assert_eq!(
                text(|assembler| assembler.shift_right_logical(target, count)),
                format!("lsr {register}, {register}, {amount}")
            );
            assert_eq!(
                text(|assembler| assembler.shift_right_arithmetic(target, count)),
                format!("asr {register}, {register}, {amount}")
            );
        }
        // A shift by nothing: `ubfm #0, #63`, which the decoder names
        // `lsr #0`.
        assert_eq!(
            text(|assembler| assembler.shift_left(target, 0)),
            format!("lsr {register}, {register}, #0x0")
        );
    }
}

#[test]
#[should_panic(expected = "shift by 64")]
fn a_shift_by_sixty_four_is_refused() {
    text(|assembler| assembler.shift_left(aarch64::X0, 64));
}

/// The instructions of a compare of `left` with a register `right`.
fn compare_lines(left: Register, right: Register) -> Vec<String> {
    if right == aarch64::SP {
        vec!["mov x16, sp".to_owned(), format!("cmp {}, x16", name(left))]
    } else {
        vec![format!("cmp {}, {}", name(left), name(right))]
    }
}

#[test]
fn a_compare_of_registers_is_a_cmp() {
    for left in allowed_or_stack() {
        for right in allowed_or_stack() {
            assert_eq!(
                text(|assembler| assembler.compare(left, right)),
                joined(&compare_lines(left, right))
            );
        }
    }
}

#[test]
fn a_compare_with_the_stack_pointer_on_the_right_goes_through_x16() {
    assert_eq!(
        text(|assembler| assembler.compare(aarch64::X3, aarch64::SP)),
        "mov x16, sp\ncmp x3, x16"
    );
    assert_eq!(
        text(|assembler| assembler.compare(aarch64::SP, aarch64::X3)),
        "cmp sp, x3"
    );
}

#[test]
fn a_compare_with_an_immediate_takes_cmp_cmn_or_x16() {
    for left in allowed_or_stack() {
        let register = name(left);
        for value in IMMEDIATES {
            let magnitude = i64::from(value).unsigned_abs();
            let mnemonic = if value < 0 { "cmn" } else { "cmp" };
            let expected = match arithmetic_operand(magnitude) {
                Some(operand) => vec![format!("{mnemonic} {register}, {operand}")],
                None => {
                    let mut lines = move_lines("x16", sign_extended(value));
                    lines.push(format!("cmp {register}, x16"));
                    lines
                }
            };
            assert_eq!(
                text(|assembler| assembler.compare_immediate(left, value)),
                joined(&expected),
                "{value}"
            );
        }
    }
}

#[test]
fn a_compare_with_memory_loads_into_x17() {
    for left in allowed_or_stack() {
        for memory in memories(allowed_or_stack()) {
            let mut expected = access_lines(Width::Word64, true, "x17", memory);
            expected.push(format!("cmp {}, x17", name(left)));
            assert_eq!(
                text(|assembler| assembler.compare_memory(left, memory)),
                joined(&expected),
                "{memory:?}"
            );
        }
    }
}

/// The instructions of a test of `left`'s `width` bits with `right`.
fn test_lines(width: Width, left: Register, right: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let (tested, scratch) = if left == aarch64::SP {
        lines.push("mov x16, sp".to_owned());
        (Register(16), "x17")
    } else {
        (left, "x16")
    };
    let (register, immediate, encodable) = if width == Width::Word64 {
        let value = sign_extended(right);
        (
            name(tested),
            hexadecimal(value),
            aarch64::logical_immediate(value).is_some(),
        )
    } else {
        let value = u64::from(right.cast_unsigned());
        (
            word_name(tested),
            hexadecimal(value),
            aarch64::logical_immediate(value | value << 32).is_some(),
        )
    };
    if encodable {
        lines.push(format!("tst {register}, {immediate}"));
    } else {
        lines.extend(move_lines(scratch, sign_extended(right)));
        let scratch = data_name(width, scratch);
        lines.push(format!("tst {register}, {scratch}"));
    }
    lines
}

#[test]
fn a_test_of_an_immediate_takes_the_bitmask_or_x16() {
    for left in allowed_or_stack() {
        for value in LOGICAL_IMMEDIATES.into_iter().chain(IMMEDIATES) {
            for width in [Width::Word64, Width::Word32] {
                assert_eq!(
                    text(|assembler| assembler.test_immediate_with_width(width, left, value)),
                    joined(&test_lines(width, left, value)),
                    "{width:?} {value:#x}"
                );
            }
        }
    }
    // The 32-bit form tests bit 31 alone.
    assert_eq!(
        text(|assembler| assembler.test_immediate_with_width(Width::Word32, aarch64::X1, i32::MIN)),
        "tst w1, #0x80000000"
    );
}

/// Every logical immediate of element size `size` (2 to 64): every run of
/// one to `size - 1` ones, rotated by every amount, replicated over 64 bits.
fn patterns(size: u32) -> Vec<(u64, u32, u32)> {
    let mask = if size == 64 {
        u64::MAX
    } else {
        (1 << size) - 1
    };
    let mut found = Vec::new();
    for ones in 1..size {
        let run = (1_u64 << ones) - 1;
        for rotation in 0..size {
            let element = if rotation == 0 {
                run
            } else {
                ((run >> rotation) | (run << (size - rotation))) & mask
            };
            let mut value = 0;
            let mut position = 0;
            while position < 64 {
                value |= element << position;
                position += size;
            }
            found.push((value, ones, rotation));
        }
    }
    found
}

#[test]
fn every_bitmask_pattern_round_trips_through_its_logical_immediate() {
    let mut count = 0;
    for size in [2, 4, 8, 16, 32, 64] {
        for (value, ones, rotation) in patterns(size) {
            let Some(encoded) = aarch64::logical_immediate(value) else {
                panic!("{value:#x} has a logical immediate");
            };
            // N:immr:imms, as the Arm manual's DecodeBitMasks reads it.
            let size_prefix = (!(size - 1) << 1) & 0x3F;
            let expected =
                (u32::from(size == 64) << 12) | (rotation << 6) | size_prefix | (ones - 1);
            assert_eq!(encoded, expected, "{value:#x}");
            // `and x0, x0, #value`, read back by the disassembler.
            let word = 0x9200_0000 | (encoded << 10);
            assert_eq!(
                disassemble(&word.to_le_bytes()),
                format!("and x0, x0, {}", hexadecimal(value))
            );
            count += 1;
        }
    }
    assert_eq!(count, 5334);
    for value in [0, u64::MAX, 0b101, 0x1234, 0b1011] {
        assert_eq!(aarch64::logical_immediate(value), None, "{value:#x}");
    }
}

#[test]
#[should_panic(expected = "reserved")]
fn a_reserved_register_x16_is_refused() {
    text(|assembler| assembler.move_register(aarch64::X0, aarch64::X16));
}

#[test]
#[should_panic(expected = "reserved")]
fn a_reserved_register_x17_is_refused() {
    text(|assembler| assembler.add(aarch64::X17, aarch64::X0));
}

#[test]
#[should_panic(expected = "reserved")]
fn a_reserved_register_x18_is_refused() {
    let memory = Memory {
        base: aarch64::X18,
        displacement: 0,
    };
    text(|assembler| assembler.load(Width::Word64, aarch64::X0, memory));
}

#[test]
#[should_panic(expected = "reserved")]
fn a_reserved_register_x30_is_refused() {
    text(|assembler| assembler.move_immediate(aarch64::X30, 1));
}

#[test]
#[should_panic(expected = "no register 32")]
fn a_register_beyond_the_stack_pointer_is_refused() {
    text(|assembler| assembler.move_register(aarch64::X0, Register(32)));
}

#[test]
#[should_panic(expected = "stack pointer")]
fn a_flag_setting_add_on_the_stack_pointer_is_refused() {
    text(|assembler| assembler.add_immediate(aarch64::SP, 16));
}

#[test]
#[should_panic(expected = "stack pointer")]
fn a_subtract_from_the_stack_pointer_is_refused() {
    text(|assembler| assembler.subtract(aarch64::SP, aarch64::X0));
}

#[test]
#[should_panic(expected = "stack pointer")]
fn a_load_into_the_stack_pointer_is_refused() {
    let memory = Memory {
        base: aarch64::X0,
        displacement: 0,
    };
    text(|assembler| assembler.load(Width::Word64, aarch64::SP, memory));
}
