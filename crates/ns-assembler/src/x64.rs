//! The x64 encoding of the operations.
//!
//! Every operation is one instruction, the shortest whose semantics are the
//! operation's: an immediate that fits a signed byte takes the byte form, a
//! displacement likewise, and a 64-bit constant the 32-bit move when its
//! upper half is zero, since that move clears it.

use crate::{BranchForm, BranchKind, Condition, Encoder, Memory, Operation, Register, Width};

pub const RAX: Register = Register(0);
pub const RCX: Register = Register(1);
pub const RDX: Register = Register(2);
pub const RBX: Register = Register(3);
pub const RSP: Register = Register(4);
pub const RBP: Register = Register(5);
pub const RSI: Register = Register(6);
pub const RDI: Register = Register(7);
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const R8: Register = Register(8);
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const R9: Register = Register(9);
pub const R10: Register = Register(10);
pub const R11: Register = Register(11);
pub const R12: Register = Register(12);
pub const R13: Register = Register(13);
pub const R14: Register = Register(14);
pub const R15: Register = Register(15);

/// The x64 encoder.
#[derive(Clone, Copy, Default, Debug)]
pub struct X64;

/// The low three bits of a register number, the part ModRM, SIB and an
/// opcode's register field hold.
const LOW_BITS: u8 = 0b111;

/// The ModRM `rm` value that announces a SIB byte, and the base number
/// (RSP, R12) that cannot be named without one.
const SIB_FOLLOWS: u8 = 0b100;

/// The SIB byte of a base alone: no index, scale one, base `rm`'s.
const SIB_BASE_ONLY: u8 = 0x24;

/// The base number (RBP, R13) that ModRM's mode 0 takes for "no base,
/// a 32-bit displacement", so it needs a displacement even of 0.
const NO_BASE_IN_MODE_ZERO: u8 = 0b101;

fn number(register: Register) -> u8 {
    assert!(
        register.0 < 16,
        "x64 has no register {}; registers are numbered 0 to 15",
        register.0
    );
    register.0
}

/// Writes the REX prefix of an instruction whose ModRM `reg` field (or
/// opcode extension) is `reg_field` and whose `rm` field, or opcode
/// register, is `rm_field`, both full register numbers. `forced` writes it
/// even when it carries nothing: a byte operand of register 4 to 7 means
/// SPL to DIL only behind a REX, AH to BH without one.
fn rex(code: &mut Vec<u8>, wide: bool, reg_field: u8, rm_field: u8, forced: bool) {
    let prefix = 0x40 | (u8::from(wide) << 3) | ((reg_field >> 3) << 2) | (rm_field >> 3);
    if prefix != 0x40 || forced {
        code.push(prefix);
    }
}

fn modrm(mode: u8, reg_field: u8, rm_field: u8) -> u8 {
    (mode << 6) | ((reg_field & LOW_BITS) << 3) | (rm_field & LOW_BITS)
}

/// `opcode` with a register `reg_field` and a register `rm_field`.
fn register_register(code: &mut Vec<u8>, opcode: &[u8], reg_field: Register, rm_field: Register) {
    let (reg_field, rm_field) = (number(reg_field), number(rm_field));
    rex(code, true, reg_field, rm_field, false);
    code.extend_from_slice(opcode);
    code.push(modrm(0b11, reg_field, rm_field));
}

/// `opcode` with a register or opcode extension `reg_field` and a memory
/// operand.
fn register_memory(
    code: &mut Vec<u8>,
    wide: bool,
    forced: bool,
    opcode: &[u8],
    reg_field: u8,
    memory: Memory,
) {
    let base = number(memory.base);
    rex(code, wide, reg_field, base, forced);
    code.extend_from_slice(opcode);
    let short_displacement = i8::try_from(memory.displacement);
    let mode = if memory.displacement == 0 && base & LOW_BITS != NO_BASE_IN_MODE_ZERO {
        0b00
    } else if short_displacement.is_ok() {
        0b01
    } else {
        0b10
    };
    code.push(modrm(mode, reg_field, base));
    if base & LOW_BITS == SIB_FOLLOWS {
        code.push(SIB_BASE_ONLY);
    }
    match (mode, short_displacement) {
        (0b01, Ok(displacement)) => code.extend_from_slice(&displacement.to_le_bytes()),
        (0b10, _) => code.extend_from_slice(&memory.displacement.to_le_bytes()),
        _ => {}
    }
}

/// An instruction of the immediate group (`83` with a sign-extended byte,
/// `81` with a doubleword), `extension` naming the operation, on a 64-bit
/// register.
fn immediate_group(code: &mut Vec<u8>, extension: u8, target: Register, value: i32) {
    let target = number(target);
    rex(code, true, 0, target, false);
    if let Ok(byte) = i8::try_from(value) {
        code.push(0x83);
        code.push(modrm(0b11, extension, target));
        code.extend_from_slice(&byte.to_le_bytes());
    } else {
        code.push(0x81);
        code.push(modrm(0b11, extension, target));
        code.extend_from_slice(&value.to_le_bytes());
    }
}

/// A shift by a constant, `C1 /extension ib`.
fn shift(code: &mut Vec<u8>, extension: u8, target: Register, count: u8) {
    assert!(count < 64, "a shift by {count} is no 64-bit shift");
    let target = number(target);
    rex(code, true, 0, target, false);
    code.push(0xC1);
    code.push(modrm(0b11, extension, target));
    code.push(count);
}

/// An instruction whose opcode holds its register, `base + register`.
fn opcode_register(code: &mut Vec<u8>, base_opcode: u8, register: Register) {
    let register = number(register);
    rex(code, false, 0, register, false);
    code.push(base_opcode + (register & LOW_BITS));
}

/// The condition code `cc` of `Jcc`.
fn condition_code(condition: Condition) -> u8 {
    match condition {
        Condition::Overflow => 0x0,
        Condition::NoOverflow => 0x1,
        Condition::Below => 0x2,
        Condition::AboveOrEqual => 0x3,
        Condition::Equal => 0x4,
        Condition::NotEqual => 0x5,
        Condition::BelowOrEqual => 0x6,
        Condition::Above => 0x7,
        Condition::Less => 0xC,
        Condition::GreaterOrEqual => 0xD,
        Condition::LessOrEqual => 0xE,
        Condition::Greater => 0xF,
    }
}

impl Encoder for X64 {
    fn encode(&self, operation: Operation, code: &mut Vec<u8>) {
        match operation {
            Operation::MoveRegister { target, source } => {
                register_register(code, &[0x89], source, target);
            }
            Operation::MoveImmediate { target, value } => {
                let register = number(target);
                if let Ok(doubleword) = u32::try_from(value) {
                    rex(code, false, 0, register, false);
                    code.push(0xB8 + (register & LOW_BITS));
                    code.extend_from_slice(&doubleword.to_le_bytes());
                } else if let Ok(signed) = i32::try_from(value.cast_signed()) {
                    rex(code, true, 0, register, false);
                    code.push(0xC7);
                    code.push(modrm(0b11, 0, register));
                    code.extend_from_slice(&signed.to_le_bytes());
                } else {
                    rex(code, true, 0, register, false);
                    code.push(0xB8 + (register & LOW_BITS));
                    code.extend_from_slice(&value.to_le_bytes());
                }
            }
            Operation::Load {
                width,
                target,
                source,
            } => {
                let target = number(target);
                // Every narrow load writes the 32-bit register, which clears
                // the upper half: the zero extension the operation promises.
                let (wide, opcode): (bool, &[u8]) = match width {
                    Width::Byte => (false, &[0x0F, 0xB6]),
                    Width::Half => (false, &[0x0F, 0xB7]),
                    Width::Word32 => (false, &[0x8B]),
                    Width::Word64 => (true, &[0x8B]),
                };
                register_memory(code, wide, false, opcode, target, source);
            }
            Operation::Store {
                width,
                target,
                source,
            } => {
                let source = number(source);
                match width {
                    Width::Byte => {
                        register_memory(
                            code,
                            false,
                            (4..8).contains(&source),
                            &[0x88],
                            source,
                            target,
                        );
                    }
                    Width::Half => {
                        code.push(0x66);
                        register_memory(code, false, false, &[0x89], source, target);
                    }
                    Width::Word32 => register_memory(code, false, false, &[0x89], source, target),
                    Width::Word64 => register_memory(code, true, false, &[0x89], source, target),
                }
            }
            Operation::StoreImmediate32 { target, value } => {
                register_memory(code, true, false, &[0xC7], 0, target);
                code.extend_from_slice(&value.to_le_bytes());
            }
            Operation::Add { target, source } => register_register(code, &[0x01], source, target),
            Operation::Subtract { target, source } => {
                register_register(code, &[0x29], source, target);
            }
            Operation::MultiplyChecked { target, source } => {
                register_register(code, &[0x0F, 0xAF], target, source);
            }
            Operation::AndImmediate { target, value } => immediate_group(code, 4, target, value),
            Operation::OrImmediate { target, value } => immediate_group(code, 1, target, value),
            Operation::ShiftLeft { target, count } => shift(code, 4, target, count),
            Operation::ShiftRightArithmetic { target, count } => shift(code, 7, target, count),
            Operation::Compare { left, right } => register_register(code, &[0x39], right, left),
            Operation::CompareImmediate { left, right } => immediate_group(code, 7, left, right),
            Operation::CompareMemory { left, right } => {
                register_memory(code, true, false, &[0x3B], number(left), right);
            }
            Operation::TestImmediate { left, right } => {
                // `test` has no sign-extended byte form.
                let left = number(left);
                rex(code, true, 0, left, false);
                code.push(0xF7);
                code.push(modrm(0b11, 0, left));
                code.extend_from_slice(&right.to_le_bytes());
            }
            Operation::CallRegister { target } => {
                let target = number(target);
                rex(code, false, 0, target, false);
                code.push(0xFF);
                code.push(modrm(0b11, 2, target));
            }
            Operation::Push { source } => opcode_register(code, 0x50, source),
            Operation::Pop { target } => opcode_register(code, 0x58, target),
            Operation::AddStack { amount } => {
                // `lea` rather than `add`, which would clobber the flags the
                // contract says this operation keeps.
                let stack = Memory {
                    base: RSP,
                    displacement: amount,
                };
                register_memory(code, true, false, &[0x8D], number(RSP), stack);
            }
            Operation::Return => code.push(0xC3),
            Operation::Trap => code.extend_from_slice(&[0x0F, 0x0B]),
        }
    }

    fn encode_branch(
        &self,
        kind: BranchKind,
        form: BranchForm,
        distance: i64,
        code: &mut Vec<u8>,
    ) -> bool {
        // x64 measures a branch from its end, so each form subtracts its
        // own size.
        match form {
            BranchForm::Short => {
                let Ok(relative) = i8::try_from(distance - 2) else {
                    return false;
                };
                code.push(match kind {
                    BranchKind::Unconditional => 0xEB,
                    BranchKind::Conditional(condition) => 0x70 + condition_code(condition),
                });
                code.extend_from_slice(&relative.to_le_bytes());
            }
            BranchForm::Long => {
                let size = match kind {
                    BranchKind::Unconditional => 5,
                    BranchKind::Conditional(_) => 6,
                };
                let Ok(relative) = i32::try_from(distance - size) else {
                    return false;
                };
                match kind {
                    BranchKind::Unconditional => code.push(0xE9),
                    BranchKind::Conditional(condition) => {
                        code.extend_from_slice(&[0x0F, 0x80 + condition_code(condition)]);
                    }
                }
                code.extend_from_slice(&relative.to_le_bytes());
            }
        }
        true
    }
}
