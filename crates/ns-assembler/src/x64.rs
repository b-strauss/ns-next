//! The x64 encoding of the operations.
//!
//! Every operation is one instruction, the shortest whose semantics are the
//! operation's: an immediate that fits a signed byte takes the byte form, a
//! displacement likewise, and a 64-bit constant the 32-bit move when its
//! upper half is zero, since that move clears it. The exceptions are the
//! signed division, whose dividend `cqo` sign-extends first, and the
//! conversion of an integer to a double, whose `cvtsi2sd` follows an
//! `xorps` of its target.
//!
//! A double is an SSE2 scalar in an `xmm` register's low 64 bits; the one
//! instruction beyond SSE2, `roundsd`, is SSE4.1's, which the x86-64-v2
//! level the binary requires includes.
//!
//! Every opcode, prefix, opcode extension and encoding field is a named
//! constant whose documentation cites the Intel 64 and IA-32 Architectures
//! Software Developer's Manual (SDM), which Intel publishes at
//! <https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html>.
//! Intel publishes no stable page per instruction, so a citation names the
//! volume and the section title as the manual prints it, and unofficial
//! mirrors are not linked. No numeric pattern appears in a function body;
//! `tests/encoding_sources.rs` holds every encoder file to that.

use crate::{
    BranchForm, BranchKind, Condition, Encoder, FloatArithmetic, FloatRegister, LabelReference,
    Memory, Operation, Register, Rounding, Width,
};

/// `rax`: register number 0, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const RAX: Register = Register(0);
/// `rcx`: register number 1, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const RCX: Register = Register(1);
/// `rdx`: register number 2, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const RDX: Register = Register(2);
/// `rbx`: register number 3, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const RBX: Register = Register(3);
/// `rsp`: register number 4, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const RSP: Register = Register(4);
/// `rbp`: register number 5, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const RBP: Register = Register(5);
/// `rsi`: register number 6, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const RSI: Register = Register(6);
/// `rdi`: register number 7, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const RDI: Register = Register(7);
/// `r8`: register number 8, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const R8: Register = Register(8);
/// `r9`: register number 9, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const R9: Register = Register(9);
/// `r10`: register number 10, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const R10: Register = Register(10);
/// `r11`: register number 11, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const R11: Register = Register(11);
/// `r12`: register number 12, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const R12: Register = Register(12);
/// `r13`: register number 13, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const R13: Register = Register(13);
/// `r14`: register number 14, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const R14: Register = Register(14);
/// `r15`: register number 15, as ModRM, SIB and opcode fields name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const R15: Register = Register(15);

/// `xmm0`: register number 0, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM0: FloatRegister = FloatRegister(0);
/// `xmm1`: register number 1, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM1: FloatRegister = FloatRegister(1);
/// `xmm2`: register number 2, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM2: FloatRegister = FloatRegister(2);
/// `xmm3`: register number 3, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM3: FloatRegister = FloatRegister(3);
/// `xmm4`: register number 4, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM4: FloatRegister = FloatRegister(4);
/// `xmm5`: register number 5, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM5: FloatRegister = FloatRegister(5);
/// `xmm6`: register number 6, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM6: FloatRegister = FloatRegister(6);
/// `xmm7`: register number 7, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM7: FloatRegister = FloatRegister(7);
/// `xmm8`: register number 8, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM8: FloatRegister = FloatRegister(8);
/// `xmm9`: register number 9, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM9: FloatRegister = FloatRegister(9);
/// `xmm10`: register number 10, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM10: FloatRegister = FloatRegister(10);
/// `xmm11`: register number 11, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM11: FloatRegister = FloatRegister(11);
/// `xmm12`: register number 12, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM12: FloatRegister = FloatRegister(12);
/// `xmm13`: register number 13, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM13: FloatRegister = FloatRegister(13);
/// `xmm14`: register number 14, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM14: FloatRegister = FloatRegister(14);
/// `xmm15`: register number 15, as the ModRM fields of an SSE
/// instruction name it.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
pub const XMM15: FloatRegister = FloatRegister(15);

/// The register `IDIV r/m64` divides and leaves its quotient in: the low
/// half of the dividend `RDX:RAX`.
///
/// Intel SDM Vol. 2A, IDIV—Signed Divide.
pub const DIVIDEND: Register = RAX;

/// The register `IDIV r/m64` takes the dividend's high half from and
/// leaves its remainder in.
///
/// Intel SDM Vol. 2A, IDIV—Signed Divide.
pub const REMAINDER: Register = RDX;

/// The x64 encoder.
#[derive(Clone, Copy, Default, Debug)]
pub struct X64;

/// The low three bits of a register number, the part ModRM, SIB and an
/// opcode's register field hold; the fourth is a REX bit.
///
/// Intel SDM Vol. 2A, 2.1.3 ModR/M and SIB Bytes.
const LOW_BITS: u8 = 0b111;

/// The number of registers: a register number is 0 to 15, its fourth bit
/// the REX bit.
///
/// Intel SDM Vol. 2A, 2.2.1 REX Prefixes.
const REGISTER_COUNT: u8 = 16;

/// The first register that is a byte register only behind a REX prefix
/// (SPL, BPL, SIL, DIL), up to but not including the eighth register.
///
/// Intel SDM Vol. 2A, 2.2.1 REX Prefixes.
const FIRST_REX_BYTE_REGISTER: u8 = 4;

/// The shift that places a ModRM `mode` field (bits 7:6).
///
/// Intel SDM Vol. 2A, 2.1.3 ModR/M and SIB Bytes.
const MODRM_MODE_SHIFT: u8 = 6;

/// The shift that places a ModRM `reg` field or opcode extension (bits 5:3).
///
/// Intel SDM Vol. 2A, 2.1.3 ModR/M and SIB Bytes.
const MODRM_REG_SHIFT: u8 = 3;

/// ModRM mode 00: memory operand, no displacement (except for base number
/// 5, see [`NO_BASE_IN_MODE_ZERO`]).
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
const MODRM_MODE_NO_DISPLACEMENT: u8 = 0b00;

/// ModRM mode 01: memory operand with an 8-bit signed displacement.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
const MODRM_MODE_BYTE_DISPLACEMENT: u8 = 0b01;

/// ModRM mode 10: memory operand with a 32-bit signed displacement.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
const MODRM_MODE_DOUBLEWORD_DISPLACEMENT: u8 = 0b10;

/// ModRM mode 11: the `rm` field names a register, not memory.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
const MODRM_MODE_REGISTER: u8 = 0b11;

/// The ModRM `rm` value that announces a SIB byte, and the base number
/// (RSP, R12) that cannot be named without one.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
const SIB_FOLLOWS: u8 = 0b100;

/// The SIB byte of a base alone: no index (`index` 100), scale one, base
/// `rm`'s (`base` 100, as [`SIB_FOLLOWS`] demands).
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
const SIB_BASE_ONLY: u8 = 0x24;

/// The base number (RBP, R13) that ModRM's mode 0 takes for "no base,
/// a 32-bit displacement", so it needs a displacement even of 0.
///
/// Intel SDM Vol. 2A, 2.1.5 Addressing-Mode Encoding of ModR/M and SIB Bytes.
const NO_BASE_IN_MODE_ZERO: u8 = 0b101;

/// The REX prefix with no bit set: bits 7:4 are always 0100.
///
/// Intel SDM Vol. 2A, 2.2.1 REX Prefixes.
const REX_BASE: u8 = 0x40;

/// REX.W: the operation is 64 bits wide.
///
/// Intel SDM Vol. 2A, 2.2.1 REX Prefixes.
const REX_WIDE: u8 = 0b1000;

/// REX.R: the fourth bit of the ModRM `reg` field.
///
/// Intel SDM Vol. 2A, 2.2.1 REX Prefixes.
const REX_REG_EXTENSION: u8 = 0b100;

/// REX.B: the fourth bit of the ModRM `rm` field, the SIB base or an
/// opcode's register.
///
/// Intel SDM Vol. 2A, 2.2.1 REX Prefixes.
const REX_RM_EXTENSION: u8 = 0b1;

/// The operand-size override prefix, which makes a store 16 bits wide.
///
/// Intel SDM Vol. 2A, 2.1.1 Instruction Prefixes.
const OPERAND_SIZE_PREFIX: u8 = 0x66;

/// The `66` prefix as the mandatory prefix of `MOVQ`, `UCOMISD` and
/// `ROUNDSD`, which selects their double-precision or quadword form; it
/// stands before the REX prefix.
///
/// Intel SDM Vol. 2A, 2.1.1 Instruction Prefixes; 2.2.1 REX Prefixes.
const DOUBLE_PRECISION_PREFIX: u8 = 0x66;

/// The `F2` prefix as the mandatory prefix of the scalar double
/// instructions (`MOVSD`, `ADDSD`, `CVTSI2SD`, …); it stands before the
/// REX prefix.
///
/// Intel SDM Vol. 2A, 2.1.1 Instruction Prefixes; 2.2.1 REX Prefixes.
const SCALAR_DOUBLE_PREFIX: u8 = 0xF2;

/// `MOVQ xmm, r/m64` (`66 REX.W 0F 6E /r`) — move a quadword into a
/// register's low half, zeroing the rest.
///
/// Intel SDM Vol. 2B, MOVD/MOVQ—Move Doubleword/Move Quadword.
const MOVE_QUADWORD_TO_FLOAT: [u8; 2] = [0x0F, 0x6E];

/// `MOVQ r/m64, xmm` (`66 REX.W 0F 7E /r`) — move a register's low
/// quadword out.
///
/// Intel SDM Vol. 2B, MOVD/MOVQ—Move Doubleword/Move Quadword.
const MOVE_QUADWORD_FROM_FLOAT: [u8; 2] = [0x0F, 0x7E];

/// `MOVAPS xmm1, xmm2` (`0F 28 /r`) — the move between `xmm` registers:
/// it copies the whole register, so unlike `MOVSD`'s register form it
/// depends on nothing of the target.
///
/// Intel SDM Vol. 2B, MOVAPS—Move Aligned Packed Single Precision
/// Floating-Point Values.
const MOVE_ALIGNED_PACKED_SINGLE: [u8; 2] = [0x0F, 0x28];

/// `MOVSD xmm1, m64` (`F2 0F 10 /r`) — load a double, zeroing the upper
/// half.
///
/// Intel SDM Vol. 2B, MOVSD—Move or Merge Scalar Double Precision
/// Floating-Point Value.
const LOAD_SCALAR_DOUBLE: [u8; 2] = [0x0F, 0x10];

/// `MOVSD m64, xmm1` (`F2 0F 11 /r`) — store a double.
///
/// Intel SDM Vol. 2B, MOVSD—Move or Merge Scalar Double Precision
/// Floating-Point Value.
const STORE_SCALAR_DOUBLE: [u8; 2] = [0x0F, 0x11];

/// `ADDSD xmm1, xmm2/m64` (`F2 0F 58 /r`).
///
/// Intel SDM Vol. 2A, ADDSD—Add Scalar Double Precision Floating-Point
/// Values.
const ADD_SCALAR_DOUBLE: [u8; 2] = [0x0F, 0x58];

/// `SUBSD xmm1, xmm2/m64` (`F2 0F 5C /r`).
///
/// Intel SDM Vol. 2B, SUBSD—Subtract Scalar Double Precision
/// Floating-Point Value.
const SUBTRACT_SCALAR_DOUBLE: [u8; 2] = [0x0F, 0x5C];

/// `MULSD xmm1, xmm2/m64` (`F2 0F 59 /r`).
///
/// Intel SDM Vol. 2B, MULSD—Multiply Scalar Double Precision
/// Floating-Point Value.
const MULTIPLY_SCALAR_DOUBLE: [u8; 2] = [0x0F, 0x59];

/// `DIVSD xmm1, xmm2/m64` (`F2 0F 5E /r`).
///
/// Intel SDM Vol. 2A, DIVSD—Divide Scalar Double Precision Floating-Point
/// Value.
const DIVIDE_SCALAR_DOUBLE: [u8; 2] = [0x0F, 0x5E];

/// `SQRTSD xmm1, xmm2/m64` (`F2 0F 51 /r`).
///
/// Intel SDM Vol. 2B, SQRTSD—Compute Square Root of Scalar Double
/// Precision Floating-Point Value.
const SQUARE_ROOT_SCALAR_DOUBLE: [u8; 2] = [0x0F, 0x51];

/// `ROUNDSD xmm1, xmm2/m64, imm8` (`66 0F 3A 0B /r ib`), SSE4.1.
///
/// Intel SDM Vol. 2B, ROUNDSD—Round Scalar Double Precision Floating-Point
/// Values.
const ROUND_SCALAR_DOUBLE: [u8; 3] = [0x0F, 0x3A, 0x0B];

/// `ROUNDSD`'s immediate rounding down: rounding control 01b (toward
/// negative infinity), bit 3 set to suppress the precision exception.
///
/// Intel SDM Vol. 2B, 4.3 ROUNDSD, Table 4-18 Rounding Modes and Encoding
/// of Rounding Control (RC) Field.
const ROUND_DOWN_IMMEDIATE: u8 = 0x09;

/// `ROUNDSD`'s immediate rounding up: rounding control 10b (toward
/// positive infinity), bit 3 set to suppress the precision exception.
///
/// Intel SDM Vol. 2B, 4.3 ROUNDSD, Table 4-18 Rounding Modes and Encoding
/// of Rounding Control (RC) Field.
const ROUND_UP_IMMEDIATE: u8 = 0x0A;

/// `ROUNDSD`'s immediate rounding toward zero: rounding control 11b
/// (truncate), bit 3 set to suppress the precision exception.
///
/// Intel SDM Vol. 2B, 4.3 ROUNDSD, Table 4-18 Rounding Modes and Encoding
/// of Rounding Control (RC) Field.
const ROUND_TOWARD_ZERO_IMMEDIATE: u8 = 0x0B;

/// `UCOMISD xmm1, xmm2/m64` (`66 0F 2E /r`) — compare two doubles into
/// ZF, PF and CF: unordered 111, less 001, equal 100, greater 000.
///
/// Intel SDM Vol. 2B, UCOMISD—Unordered Compare Scalar Double Precision
/// Floating-Point Values and Set EFLAGS.
const UNORDERED_COMPARE_SCALAR_DOUBLE: [u8; 2] = [0x0F, 0x2E];

/// `XORPS xmm1, xmm2/m128` (`0F 57 /r`) — of a register with itself, the
/// zero that breaks `CVTSI2SD`'s dependency on its target.
///
/// Intel SDM Vol. 2C, XORPS—Bitwise Logical XOR of Packed Single
/// Precision Floating-Point Values.
const XOR_PACKED_SINGLE: [u8; 2] = [0x0F, 0x57];

/// `CVTSI2SD xmm1, r/m64` (`F2 REX.W 0F 2A /r`) — convert a signed
/// quadword to a double, rounded as MXCSR says: to nearest, ties to even.
///
/// Intel SDM Vol. 2A, CVTSI2SD—Convert Doubleword Integer to Scalar Double
/// Precision Floating-Point Value.
const CONVERT_INTEGER_TO_SCALAR_DOUBLE: [u8; 2] = [0x0F, 0x2A];

/// `CVTTSD2SI r64, xmm1/m64` (`F2 REX.W 0F 2C /r`) — convert a double to
/// a signed quadword, truncated toward zero.
///
/// Intel SDM Vol. 2A, CVTTSD2SI—Convert With Truncation Scalar Double
/// Precision Floating-Point Value to Signed Integer.
const CONVERT_TRUNCATED_SCALAR_DOUBLE_TO_INTEGER: [u8; 2] = [0x0F, 0x2C];

/// `MOV r/m64, r64` (`89 /r`) — move a register to a register or memory;
/// also the 32-bit and, behind [`OPERAND_SIZE_PREFIX`], 16-bit store.
///
/// Intel SDM Vol. 2B, MOV—Move.
const MOVE_REGISTER_TO_RM: [u8; 1] = [0x89];

/// `MOV r/m8, r8` (`88 /r`) — the byte store.
///
/// Intel SDM Vol. 2B, MOV—Move.
const MOVE_BYTE_REGISTER_TO_RM: [u8; 1] = [0x88];

/// `MOV r64, r/m64` (`8B /r`) — the 32- and 64-bit load.
///
/// Intel SDM Vol. 2B, MOV—Move.
const MOVE_RM_TO_REGISTER: [u8; 1] = [0x8B];

/// `MOV r32, imm32` and `MOV r64, imm64` (`B8+rd`, REX.W for the latter) —
/// the opcode whose low three bits are the register.
///
/// Intel SDM Vol. 2B, MOV—Move.
const MOVE_IMMEDIATE_TO_REGISTER_BASE: u8 = 0xB8;

/// `MOV r/m64, imm32` (`C7 /0 id`) — move a sign-extended doubleword to a
/// register or to memory.
///
/// Intel SDM Vol. 2B, MOV—Move.
const MOVE_IMMEDIATE_TO_RM: [u8; 1] = [0xC7];

/// The `/0` opcode extension of [`MOVE_IMMEDIATE_TO_RM`].
///
/// Intel SDM Vol. 2B, MOV—Move.
const MOVE_IMMEDIATE_EXTENSION: u8 = 0;

/// `MOVZX r32, r/m8` (`0F B6 /r`) — the byte load, zero-extended.
///
/// Intel SDM Vol. 2B, MOVZX—Move With Zero-Extend.
const MOVE_ZERO_EXTENDED_BYTE: [u8; 2] = [0x0F, 0xB6];

/// `MOVZX r32, r/m16` (`0F B7 /r`) — the 16-bit load, zero-extended.
///
/// Intel SDM Vol. 2B, MOVZX—Move With Zero-Extend.
const MOVE_ZERO_EXTENDED_HALF: [u8; 2] = [0x0F, 0xB7];

/// `ADD r/m64, r64` (`01 /r`) — add a register to a register.
///
/// Intel SDM Vol. 2A, ADD—Add.
const ADD_REGISTER_TO_RM: [u8; 1] = [0x01];

/// `CMOVcc r64, r/m64` (`0F 40+cc /r`) — the escape byte of the
/// conditional move.
///
/// Intel SDM Vol. 2A, CMOVcc—Conditional Move.
const CONDITIONAL_MOVE_ESCAPE: u8 = 0x0F;

/// The second opcode byte of `CMOVcc r64, r/m64`, to which a condition
/// code is added.
///
/// Intel SDM Vol. 2A, CMOVcc—Conditional Move.
const CONDITIONAL_MOVE_BASE: u8 = 0x40;

/// `CQO` (`REX.W 99`) — sign-extends `RAX` into `RDX:RAX`, the dividend
/// of a 64-bit `IDIV`.
///
/// Intel SDM Vol. 2A, CWD/CDQ/CQO—Convert Word to Doubleword/Convert
/// Doubleword to Quadword.
const CONVERT_QUADWORD_TO_OCTWORD: u8 = 0x99;

/// `IDIV r/m64` (`F7 /7`) — signed divide of `RDX:RAX`, the quotient into
/// `RAX`, the remainder into `RDX`.
///
/// Intel SDM Vol. 2A, IDIV—Signed Divide.
const SIGNED_DIVIDE: [u8; 1] = [0xF7];

/// The `/7` extension of [`SIGNED_DIVIDE`].
///
/// Intel SDM Vol. 2A, IDIV—Signed Divide.
const SIGNED_DIVIDE_EXTENSION: u8 = 7;

/// `AND r/m64, r64` (`21 /r`) — AND a register into a register.
///
/// Intel SDM Vol. 2A, AND—Logical AND.
const AND_REGISTER_TO_RM: [u8; 1] = [0x21];

/// `SUB r/m64, r64` (`29 /r`) — subtract a register from a register.
///
/// Intel SDM Vol. 2B, SUB—Subtract.
const SUBTRACT_REGISTER_FROM_RM: [u8; 1] = [0x29];

/// `IMUL r64, r/m64` (`0F AF /r`) — signed multiply that sets OF on
/// overflow, the overflow check of the checked multiply.
///
/// Intel SDM Vol. 2A, IMUL—Signed Multiply.
const MULTIPLY_SIGNED: [u8; 2] = [0x0F, 0xAF];

/// `CMP r/m64, r64` (`39 /r`) — compare a register with a register.
///
/// Intel SDM Vol. 2A, CMP—Compare Two Operands.
const COMPARE_RM_WITH_REGISTER: [u8; 1] = [0x39];

/// `CMP r64, r/m64` (`3B /r`) — compare a register with memory.
///
/// Intel SDM Vol. 2A, CMP—Compare Two Operands.
const COMPARE_REGISTER_WITH_RM: [u8; 1] = [0x3B];

/// `TEST r/m64, imm32` (`F7 /0 id`) — AND a doubleword with a register for
/// the flags only; without REX.W, `TEST r/m32, imm32`, of the low 32 bits.
/// It has no sign-extended byte form.
///
/// Intel SDM Vol. 2B, TEST—Logical Compare.
const TEST_IMMEDIATE: [u8; 1] = [0xF7];

/// The `/0` opcode extension of [`TEST_IMMEDIATE`].
///
/// Intel SDM Vol. 2B, TEST—Logical Compare.
const TEST_IMMEDIATE_EXTENSION: u8 = 0;

/// The immediate group with a sign-extended byte (`83 /digit ib`): `ADD`,
/// `OR`, `AND`, `CMP` and others by the opcode extension.
///
/// Intel SDM Vol. 2A, AND—Logical AND (the form `AND r/m64, imm8`; `OR`
/// and `CMP` have the same opcode with their own `/digit`).
const GROUP_ONE_BYTE_IMMEDIATE: u8 = 0x83;

/// The immediate group with a doubleword (`81 /digit id`), the form of
/// [`GROUP_ONE_BYTE_IMMEDIATE`] whose immediate does not fit a byte.
///
/// Intel SDM Vol. 2A, AND—Logical AND (the form `AND r/m64, imm32`).
const GROUP_ONE_DOUBLEWORD_IMMEDIATE: u8 = 0x81;

/// The `/0` extension of the immediate group: `ADD r/m64, imm`.
///
/// Intel SDM Vol. 2A, ADD—Add.
const GROUP_ONE_ADD: u8 = 0;

/// The `/1` extension of the immediate group: `OR r/m64, imm`.
///
/// Intel SDM Vol. 2B, OR—Logical Inclusive OR.
const GROUP_ONE_OR: u8 = 1;

/// The `/4` extension of the immediate group: `AND r/m64, imm`.
///
/// Intel SDM Vol. 2A, AND—Logical AND.
const GROUP_ONE_AND: u8 = 4;

/// The `/6` extension of the immediate group: `XOR r/m64, imm`.
///
/// Intel SDM Vol. 2C, XOR—Logical Exclusive OR.
const GROUP_ONE_XOR: u8 = 6;

/// The `/5` extension of the immediate group: `SUB r/m64, imm`.
///
/// Intel SDM Vol. 2B, SUB—Subtract.
const GROUP_ONE_SUBTRACT: u8 = 5;

/// The `/7` extension of the immediate group: `CMP r/m64, imm`.
///
/// Intel SDM Vol. 2A, CMP—Compare Two Operands.
const GROUP_ONE_COMPARE: u8 = 7;

/// The shift of a register by a byte count (`C1 /digit ib`), which the
/// opcode extension turns into a left or an arithmetic right shift.
///
/// Intel SDM Vol. 2B, SAL/SAR/SHL/SHR—Shift.
const SHIFT_BY_IMMEDIATE: u8 = 0xC1;

/// The `/0` extension of [`SHIFT_BY_IMMEDIATE`]: `ROL r/m64, imm8`.
///
/// Intel SDM Vol. 2B, RCL/RCR/ROL/ROR—Rotate.
const ROTATE_LEFT_EXTENSION: u8 = 0;

/// The `/1` extension of [`SHIFT_BY_IMMEDIATE`]: `ROR r/m64, imm8`.
///
/// Intel SDM Vol. 2B, RCL/RCR/ROL/ROR—Rotate.
const ROTATE_RIGHT_EXTENSION: u8 = 1;

/// The `/4` extension of [`SHIFT_BY_IMMEDIATE`]: `SHL r/m64, imm8`.
///
/// Intel SDM Vol. 2B, SAL/SAR/SHL/SHR—Shift.
const SHIFT_LEFT_EXTENSION: u8 = 4;

/// The `/5` extension of [`SHIFT_BY_IMMEDIATE`]: `SHR r/m64, imm8`.
///
/// Intel SDM Vol. 2B, SAL/SAR/SHL/SHR—Shift.
const SHIFT_RIGHT_LOGICAL_EXTENSION: u8 = 5;

/// The `/7` extension of [`SHIFT_BY_IMMEDIATE`]: `SAR r/m64, imm8`.
///
/// Intel SDM Vol. 2B, SAL/SAR/SHL/SHR—Shift.
const SHIFT_RIGHT_ARITHMETIC_EXTENSION: u8 = 7;

/// The widest 64-bit shift count plus one: a shift of 64 or more masks
/// its count, which is never what the operation means.
///
/// Intel SDM Vol. 2B, SAL/SAR/SHL/SHR—Shift.
const SHIFT_COUNT_LIMIT: u8 = 64;

/// `CALL r/m64` and `JMP r/m64` (`FF /2`, `FF /4`) — the opcode shared by
/// the indirect call and the indirect jump.
///
/// Intel SDM Vol. 2A, CALL—Call Procedure; JMP—Jump.
const INDIRECT_BRANCH: u8 = 0xFF;

/// The `/2` extension of [`INDIRECT_BRANCH`]: `CALL r/m64`, near, absolute
/// indirect.
///
/// Intel SDM Vol. 2A, CALL—Call Procedure.
const INDIRECT_CALL_EXTENSION: u8 = 2;

/// The `/4` extension of [`INDIRECT_BRANCH`]: `JMP r/m64`, near, absolute
/// indirect.
///
/// Intel SDM Vol. 2A, JMP—Jump.
const INDIRECT_JUMP_EXTENSION: u8 = 4;

/// `PUSH r64` (`50+rd`) — the opcode whose low three bits are the register.
///
/// Intel SDM Vol. 2B, PUSH—Push Word, Doubleword, or Quadword Onto the Stack.
const PUSH_REGISTER_BASE: u8 = 0x50;

/// `POP r64` (`58+rd`) — the opcode whose low three bits are the register.
///
/// Intel SDM Vol. 2B, POP—Pop a Value From the Stack.
const POP_REGISTER_BASE: u8 = 0x58;

/// `LEA r64, m` (`8D /r`) — load an effective address without touching the
/// flags.
///
/// Intel SDM Vol. 2A, LEA—Load Effective Address.
const LOAD_EFFECTIVE_ADDRESS: [u8; 1] = [0x8D];

/// The ModRM `r/m` field that, in mode 0 in 64-bit mode, addresses the
/// instruction pointer of the next instruction plus a 32-bit displacement:
/// RIP-relative addressing, which takes the place of mode 0's "no base".
///
/// Intel SDM Vol. 2A, 2.2.1.6 RIP-Relative Addressing.
const RIP_RELATIVE: u8 = 0b101;

/// The size of a RIP-relative `MOV r64, m64` or `LEA r64, m`: the REX.W
/// prefix, the one-byte opcode, the ModRM byte and the 32-bit
/// displacement, which counts from the end of the instruction.
///
/// Intel SDM Vol. 2A, 2.2.1.6 RIP-Relative Addressing.
const LABEL_REFERENCE_SIZE: i64 = 7;

/// The bytes `push` and `call` move the stack pointer by in 64-bit mode.
///
/// Intel SDM Vol. 2B, PUSH—Push Word, Doubleword, or Quadword Onto the Stack
/// (in 64-bit mode the stack pointer moves by 8); Vol. 1, 6.2.2 Stack Alignment.
const STACK_SLOT_BYTES: i32 = 8;

/// `RET` (`C3`) — near return.
///
/// Intel SDM Vol. 2B, RET—Return From Procedure.
const RETURN_NEAR: u8 = 0xC3;

/// `UD2` (`0F 0B`) — the instruction that always raises an invalid-opcode
/// exception, the trap.
///
/// Intel SDM Vol. 2B, UD—Undefined Instruction.
const UNDEFINED_INSTRUCTION: [u8; 2] = [0x0F, 0x0B];

/// `INT3` (`CC`) — the one-byte breakpoint, which raises a breakpoint
/// exception at whichever byte execution enters a run of them.
///
/// Intel SDM Vol. 2A, INT n/INTO/INT3/INT1—Call to Interrupt Procedure.
const BREAKPOINT: u8 = 0xCC;

/// `JMP rel8` (`EB cb`) — the short unconditional jump.
///
/// Intel SDM Vol. 2A, JMP—Jump.
const JUMP_SHORT: u8 = 0xEB;

/// `JMP rel32` (`E9 cd`) — the near unconditional jump.
///
/// Intel SDM Vol. 2A, JMP—Jump.
const JUMP_NEAR: u8 = 0xE9;

/// `Jcc rel8` (`70+cc cb`) — the short conditional jump's opcode, to which
/// a condition code is added.
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const JUMP_SHORT_IF_BASE: u8 = 0x70;

/// The escape byte of `Jcc rel32` (`0F 80+cc cd`), the near conditional
/// jump.
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const JUMP_NEAR_IF_ESCAPE: u8 = 0x0F;

/// The second opcode byte of `Jcc rel32`, to which a condition code is
/// added.
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const JUMP_NEAR_IF_BASE: u8 = 0x80;

/// The condition code `cc` of `JO` (overflow, OF=1).
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const CONDITION_OVERFLOW: u8 = 0x0;

/// The condition code `cc` of `JNO` (OF=0).
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const CONDITION_NO_OVERFLOW: u8 = 0x1;

/// The condition code `cc` of `JB` (unsigned below, CF=1).
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const CONDITION_BELOW: u8 = 0x2;

/// The condition code `cc` of `JAE` (unsigned above or equal, CF=0).
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const CONDITION_ABOVE_OR_EQUAL: u8 = 0x3;

/// The condition code `cc` of `JE` (equal, ZF=1).
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const CONDITION_EQUAL: u8 = 0x4;

/// The condition code `cc` of `JNE` (ZF=0).
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const CONDITION_NOT_EQUAL: u8 = 0x5;

/// The condition code `cc` of `JBE` (unsigned below or equal, CF=1 or ZF=1).
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const CONDITION_BELOW_OR_EQUAL: u8 = 0x6;

/// The condition code `cc` of `JA` (unsigned above, CF=0 and ZF=0).
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const CONDITION_ABOVE: u8 = 0x7;

/// The condition code `cc` of `JP` (parity, PF=1): after `UCOMISD`,
/// unordered.
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const CONDITION_PARITY: u8 = 0xA;

/// The condition code `cc` of `JNP` (PF=0): after `UCOMISD`, ordered.
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const CONDITION_NO_PARITY: u8 = 0xB;

/// The condition code `cc` of `JL` (signed less, SF≠OF).
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const CONDITION_LESS: u8 = 0xC;

/// The condition code `cc` of `JGE` (signed greater or equal, SF=OF).
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const CONDITION_GREATER_OR_EQUAL: u8 = 0xD;

/// The condition code `cc` of `JLE` (signed less or equal, ZF=1 or SF≠OF).
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const CONDITION_LESS_OR_EQUAL: u8 = 0xE;

/// The condition code `cc` of `JG` (signed greater, ZF=0 and SF=OF).
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const CONDITION_GREATER: u8 = 0xF;

/// The length in bytes of [`JUMP_SHORT`] and of `Jcc rel8`: opcode and
/// 8-bit displacement. A displacement is measured from the instruction's end.
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const SHORT_BRANCH_SIZE: i64 = 2;

/// The length in bytes of [`JUMP_NEAR`]: opcode and 32-bit displacement.
///
/// Intel SDM Vol. 2A, JMP—Jump.
const JUMP_NEAR_SIZE: i64 = 5;

/// The length in bytes of `Jcc rel32`: two opcode bytes and a 32-bit
/// displacement.
///
/// Intel SDM Vol. 2A, Jcc—Jump if Condition Is Met.
const JUMP_NEAR_IF_SIZE: i64 = 6;

fn number(register: Register) -> u8 {
    assert!(
        register.0 < REGISTER_COUNT,
        "x64 has no register {}; registers are numbered 0 to 15",
        register.0
    );
    register.0
}

fn float_number(register: FloatRegister) -> u8 {
    assert!(
        register.0 < REGISTER_COUNT,
        "x64 has no float register {}; xmm registers are numbered 0 to 15",
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
    let mut prefix = REX_BASE;
    if wide {
        prefix |= REX_WIDE;
    }
    if reg_field > LOW_BITS {
        prefix |= REX_REG_EXTENSION;
    }
    if rm_field > LOW_BITS {
        prefix |= REX_RM_EXTENSION;
    }
    if prefix != REX_BASE || forced {
        code.push(prefix);
    }
}

fn modrm(mode: u8, reg_field: u8, rm_field: u8) -> u8 {
    (mode << MODRM_MODE_SHIFT) | ((reg_field & LOW_BITS) << MODRM_REG_SHIFT) | (rm_field & LOW_BITS)
}

/// `opcode` with a register `reg_field` and a register `rm_field`.
fn register_register(code: &mut Vec<u8>, opcode: &[u8], reg_field: Register, rm_field: Register) {
    let (reg_field, rm_field) = (number(reg_field), number(rm_field));
    rex(code, true, reg_field, rm_field, false);
    code.extend_from_slice(opcode);
    code.push(modrm(MODRM_MODE_REGISTER, reg_field, rm_field));
}

/// An SSE instruction on two registers, full register numbers: its
/// mandatory `prefix`, if any, before the REX prefix, then `opcode`.
fn prefixed_register_register(
    code: &mut Vec<u8>,
    prefix: Option<u8>,
    wide: bool,
    opcode: &[u8],
    reg_field: u8,
    rm_field: u8,
) {
    if let Some(prefix) = prefix {
        code.push(prefix);
    }
    rex(code, wide, reg_field, rm_field, false);
    code.extend_from_slice(opcode);
    code.push(modrm(MODRM_MODE_REGISTER, reg_field, rm_field));
}

/// A scalar double instruction (`F2`) whose operands are the `xmm`
/// registers `reg_field` and `rm_field`.
fn scalar_double(
    code: &mut Vec<u8>,
    opcode: &[u8],
    reg_field: FloatRegister,
    rm_field: FloatRegister,
) {
    prefixed_register_register(
        code,
        Some(SCALAR_DOUBLE_PREFIX),
        false,
        opcode,
        float_number(reg_field),
        float_number(rm_field),
    );
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
        MODRM_MODE_NO_DISPLACEMENT
    } else if short_displacement.is_ok() {
        MODRM_MODE_BYTE_DISPLACEMENT
    } else {
        MODRM_MODE_DOUBLEWORD_DISPLACEMENT
    };
    code.push(modrm(mode, reg_field, base));
    if base & LOW_BITS == SIB_FOLLOWS {
        code.push(SIB_BASE_ONLY);
    }
    match (mode, short_displacement) {
        (MODRM_MODE_BYTE_DISPLACEMENT, Ok(displacement)) => {
            code.extend_from_slice(&displacement.to_le_bytes());
        }
        (MODRM_MODE_DOUBLEWORD_DISPLACEMENT, _) => {
            code.extend_from_slice(&memory.displacement.to_le_bytes());
        }
        _ => {}
    }
}

/// An instruction of the immediate group (a sign-extended byte, or a
/// doubleword), `extension` naming the operation, on a 64-bit register.
fn immediate_group(code: &mut Vec<u8>, extension: u8, target: Register, value: i32) {
    let target = number(target);
    rex(code, true, 0, target, false);
    if let Ok(byte) = i8::try_from(value) {
        code.push(GROUP_ONE_BYTE_IMMEDIATE);
        code.push(modrm(MODRM_MODE_REGISTER, extension, target));
        code.extend_from_slice(&byte.to_le_bytes());
    } else {
        code.push(GROUP_ONE_DOUBLEWORD_IMMEDIATE);
        code.push(modrm(MODRM_MODE_REGISTER, extension, target));
        code.extend_from_slice(&value.to_le_bytes());
    }
}

/// A shift by a constant, `C1 /extension ib`.
fn shift(code: &mut Vec<u8>, extension: u8, target: Register, count: u8) {
    assert!(
        count < SHIFT_COUNT_LIMIT,
        "a shift by {count} is no 64-bit shift"
    );
    let target = number(target);
    rex(code, true, 0, target, false);
    code.push(SHIFT_BY_IMMEDIATE);
    code.push(modrm(MODRM_MODE_REGISTER, extension, target));
    code.push(count);
}

/// An instruction whose opcode holds its register, `base + register`.
fn opcode_register(code: &mut Vec<u8>, base_opcode: u8, register: Register) {
    let register = number(register);
    rex(code, false, 0, register, false);
    code.push(base_opcode + (register & LOW_BITS));
}

/// An indirect branch through a register, `FF /extension`.
fn indirect_branch(code: &mut Vec<u8>, extension: u8, target: Register) {
    let target = number(target);
    rex(code, false, 0, target, false);
    code.push(INDIRECT_BRANCH);
    code.push(modrm(MODRM_MODE_REGISTER, extension, target));
}

/// The condition code `cc` of `Jcc`.
fn condition_code(condition: Condition) -> u8 {
    match condition {
        Condition::Overflow => CONDITION_OVERFLOW,
        Condition::NoOverflow => CONDITION_NO_OVERFLOW,
        Condition::Below => CONDITION_BELOW,
        // `UCOMISD` sets CF for less and unordered, ZF for equal and
        // unordered, so "neither" -- above -- is greater and false when
        // unordered, and CF clear greater or equal.
        Condition::AboveOrEqual | Condition::FloatGreaterOrEqual => CONDITION_ABOVE_OR_EQUAL,
        Condition::Equal => CONDITION_EQUAL,
        Condition::NotEqual => CONDITION_NOT_EQUAL,
        Condition::BelowOrEqual => CONDITION_BELOW_OR_EQUAL,
        Condition::Above | Condition::FloatGreater => CONDITION_ABOVE,
        Condition::Unordered => CONDITION_PARITY,
        Condition::Ordered => CONDITION_NO_PARITY,
        Condition::Less => CONDITION_LESS,
        Condition::GreaterOrEqual => CONDITION_GREATER_OR_EQUAL,
        Condition::LessOrEqual => CONDITION_LESS_OR_EQUAL,
        Condition::Greater => CONDITION_GREATER,
    }
}

impl Encoder for X64 {
    fn stack_slot_bytes(&self) -> i32 {
        STACK_SLOT_BYTES
    }

    fn instruction_alignment(&self) -> u32 {
        // Instructions have any address.
        1
    }

    fn fill_with_stops(&self, length: usize, code: &mut Vec<u8>) {
        code.resize(code.len() + length, BREAKPOINT);
    }

    fn encode(&self, operation: Operation, code: &mut Vec<u8>) {
        match operation {
            Operation::MoveRegister { target, source } => {
                register_register(code, &MOVE_REGISTER_TO_RM, source, target);
            }
            Operation::MoveIf {
                condition,
                target,
                source,
            } => {
                register_register(
                    code,
                    &[
                        CONDITIONAL_MOVE_ESCAPE,
                        CONDITIONAL_MOVE_BASE + condition_code(condition),
                    ],
                    target,
                    source,
                );
            }
            Operation::MoveImmediate { target, value } => {
                let register = number(target);
                if let Ok(doubleword) = u32::try_from(value) {
                    rex(code, false, 0, register, false);
                    code.push(MOVE_IMMEDIATE_TO_REGISTER_BASE + (register & LOW_BITS));
                    code.extend_from_slice(&doubleword.to_le_bytes());
                } else if let Ok(signed) = i32::try_from(value.cast_signed()) {
                    rex(code, true, 0, register, false);
                    code.extend_from_slice(&MOVE_IMMEDIATE_TO_RM);
                    code.push(modrm(
                        MODRM_MODE_REGISTER,
                        MOVE_IMMEDIATE_EXTENSION,
                        register,
                    ));
                    code.extend_from_slice(&signed.to_le_bytes());
                } else {
                    rex(code, true, 0, register, false);
                    code.push(MOVE_IMMEDIATE_TO_REGISTER_BASE + (register & LOW_BITS));
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
                    Width::Byte => (false, &MOVE_ZERO_EXTENDED_BYTE),
                    Width::Half => (false, &MOVE_ZERO_EXTENDED_HALF),
                    Width::Word32 => (false, &MOVE_RM_TO_REGISTER),
                    Width::Word64 => (true, &MOVE_RM_TO_REGISTER),
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
                            (FIRST_REX_BYTE_REGISTER..=LOW_BITS).contains(&source),
                            &MOVE_BYTE_REGISTER_TO_RM,
                            source,
                            target,
                        );
                    }
                    Width::Half => {
                        code.push(OPERAND_SIZE_PREFIX);
                        register_memory(code, false, false, &MOVE_REGISTER_TO_RM, source, target);
                    }
                    Width::Word32 => {
                        register_memory(code, false, false, &MOVE_REGISTER_TO_RM, source, target);
                    }
                    Width::Word64 => {
                        register_memory(code, true, false, &MOVE_REGISTER_TO_RM, source, target);
                    }
                }
            }
            Operation::StoreImmediate32 { target, value } => {
                register_memory(
                    code,
                    true,
                    false,
                    &MOVE_IMMEDIATE_TO_RM,
                    MOVE_IMMEDIATE_EXTENSION,
                    target,
                );
                code.extend_from_slice(&value.to_le_bytes());
            }
            Operation::SubtractFromMemory { target, value } => {
                // `sub` sets the flags as `cmp` of the word before with
                // the immediate does (SDM Vol. 2B, SUB—Subtract, "Flags
                // Affected").
                if let Ok(byte) = i8::try_from(value) {
                    register_memory(
                        code,
                        true,
                        false,
                        &[GROUP_ONE_BYTE_IMMEDIATE],
                        GROUP_ONE_SUBTRACT,
                        target,
                    );
                    code.extend_from_slice(&byte.to_le_bytes());
                } else {
                    register_memory(
                        code,
                        true,
                        false,
                        &[GROUP_ONE_DOUBLEWORD_IMMEDIATE],
                        GROUP_ONE_SUBTRACT,
                        target,
                    );
                    code.extend_from_slice(&value.to_le_bytes());
                }
            }
            Operation::Add { target, source } => {
                register_register(code, &ADD_REGISTER_TO_RM, source, target);
            }
            Operation::AddImmediate { target, value } => {
                immediate_group(code, GROUP_ONE_ADD, target, value);
            }
            Operation::Subtract { target, source } => {
                register_register(code, &SUBTRACT_REGISTER_FROM_RM, source, target);
            }
            Operation::MultiplyChecked { target, source } => {
                register_register(code, &MULTIPLY_SIGNED, target, source);
            }
            Operation::DivideSigned {
                dividend,
                remainder,
                divisor,
            } => {
                assert!(
                    dividend == DIVIDEND && remainder == REMAINDER,
                    "x64 divides rax and leaves the remainder in rdx"
                );
                assert!(
                    divisor != DIVIDEND && divisor != REMAINDER,
                    "x64's divisor cannot be rax or rdx, which the division writes"
                );
                code.push(REX_BASE | REX_WIDE);
                code.push(CONVERT_QUADWORD_TO_OCTWORD);
                let divisor = number(divisor);
                rex(code, true, 0, divisor, false);
                code.extend_from_slice(&SIGNED_DIVIDE);
                code.push(modrm(MODRM_MODE_REGISTER, SIGNED_DIVIDE_EXTENSION, divisor));
            }
            Operation::And { target, source } => {
                register_register(code, &AND_REGISTER_TO_RM, source, target);
            }
            Operation::AndImmediate { target, value } => {
                immediate_group(code, GROUP_ONE_AND, target, value);
            }
            Operation::OrImmediate { target, value } => {
                immediate_group(code, GROUP_ONE_OR, target, value);
            }
            Operation::ShiftLeft { target, count } => {
                shift(code, SHIFT_LEFT_EXTENSION, target, count);
            }
            Operation::ShiftRightArithmetic { target, count } => {
                shift(code, SHIFT_RIGHT_ARITHMETIC_EXTENSION, target, count);
            }
            Operation::ShiftRightLogical { target, count } => {
                shift(code, SHIFT_RIGHT_LOGICAL_EXTENSION, target, count);
            }
            Operation::XorImmediate { target, value } => {
                immediate_group(code, GROUP_ONE_XOR, target, value);
            }
            Operation::RotateLeft { target, count } => {
                shift(code, ROTATE_LEFT_EXTENSION, target, count);
            }
            Operation::RotateRight { target, count } => {
                shift(code, ROTATE_RIGHT_EXTENSION, target, count);
            }
            Operation::MoveToFloat { target, source } => {
                prefixed_register_register(
                    code,
                    Some(DOUBLE_PRECISION_PREFIX),
                    true,
                    &MOVE_QUADWORD_TO_FLOAT,
                    float_number(target),
                    number(source),
                );
            }
            Operation::MoveFromFloat { target, source } => {
                prefixed_register_register(
                    code,
                    Some(DOUBLE_PRECISION_PREFIX),
                    true,
                    &MOVE_QUADWORD_FROM_FLOAT,
                    float_number(source),
                    number(target),
                );
            }
            Operation::MoveFloat { target, source } => {
                prefixed_register_register(
                    code,
                    None,
                    false,
                    &MOVE_ALIGNED_PACKED_SINGLE,
                    float_number(target),
                    float_number(source),
                );
            }
            Operation::LoadFloat { target, source } => {
                code.push(SCALAR_DOUBLE_PREFIX);
                register_memory(
                    code,
                    false,
                    false,
                    &LOAD_SCALAR_DOUBLE,
                    float_number(target),
                    source,
                );
            }
            Operation::StoreFloat { target, source } => {
                code.push(SCALAR_DOUBLE_PREFIX);
                register_memory(
                    code,
                    false,
                    false,
                    &STORE_SCALAR_DOUBLE,
                    float_number(source),
                    target,
                );
            }
            Operation::FloatArithmetic {
                operation,
                target,
                source,
            } => {
                let opcode = match operation {
                    FloatArithmetic::Add => &ADD_SCALAR_DOUBLE,
                    FloatArithmetic::Subtract => &SUBTRACT_SCALAR_DOUBLE,
                    FloatArithmetic::Multiply => &MULTIPLY_SCALAR_DOUBLE,
                    FloatArithmetic::Divide => &DIVIDE_SCALAR_DOUBLE,
                };
                scalar_double(code, opcode, target, source);
            }
            Operation::FloatSquareRoot { target, source } => {
                scalar_double(code, &SQUARE_ROOT_SCALAR_DOUBLE, target, source);
            }
            Operation::FloatRound {
                rounding,
                target,
                source,
            } => {
                prefixed_register_register(
                    code,
                    Some(DOUBLE_PRECISION_PREFIX),
                    false,
                    &ROUND_SCALAR_DOUBLE,
                    float_number(target),
                    float_number(source),
                );
                code.push(match rounding {
                    Rounding::Down => ROUND_DOWN_IMMEDIATE,
                    Rounding::Up => ROUND_UP_IMMEDIATE,
                    Rounding::TowardZero => ROUND_TOWARD_ZERO_IMMEDIATE,
                });
            }
            Operation::FloatCompare { left, right } => {
                prefixed_register_register(
                    code,
                    Some(DOUBLE_PRECISION_PREFIX),
                    false,
                    &UNORDERED_COMPARE_SCALAR_DOUBLE,
                    float_number(left),
                    float_number(right),
                );
            }
            Operation::IntegerToFloat { target, source } => {
                // `cvtsi2sd` writes only the low half and so waits for
                // whatever last wrote the target; zeroing it first breaks
                // that dependency.
                let target = float_number(target);
                prefixed_register_register(code, None, false, &XOR_PACKED_SINGLE, target, target);
                prefixed_register_register(
                    code,
                    Some(SCALAR_DOUBLE_PREFIX),
                    true,
                    &CONVERT_INTEGER_TO_SCALAR_DOUBLE,
                    target,
                    number(source),
                );
            }
            Operation::FloatToIntegerTruncated { target, source } => {
                prefixed_register_register(
                    code,
                    Some(SCALAR_DOUBLE_PREFIX),
                    true,
                    &CONVERT_TRUNCATED_SCALAR_DOUBLE_TO_INTEGER,
                    number(target),
                    float_number(source),
                );
            }
            Operation::Compare { left, right } => {
                register_register(code, &COMPARE_RM_WITH_REGISTER, right, left);
            }
            Operation::CompareImmediate { left, right } => {
                immediate_group(code, GROUP_ONE_COMPARE, left, right);
            }
            Operation::CompareMemory { left, right } => {
                register_memory(
                    code,
                    true,
                    false,
                    &COMPARE_REGISTER_WITH_RM,
                    number(left),
                    right,
                );
            }
            Operation::TestImmediate { width, left, right } => {
                // `test` has no sign-extended byte form; the 32-bit one
                // differs only by the missing REX.W.
                let left = number(left);
                rex(code, width == Width::Word64, 0, left, false);
                code.extend_from_slice(&TEST_IMMEDIATE);
                code.push(modrm(MODRM_MODE_REGISTER, TEST_IMMEDIATE_EXTENSION, left));
                code.extend_from_slice(&right.to_le_bytes());
            }
            // `call` pushes the return address where `EnterCalled` finds it,
            // whatever the callee begins with.
            Operation::CallRegister { target } | Operation::CallUnentered { target } => {
                indirect_branch(code, INDIRECT_CALL_EXTENSION, target);
            }
            Operation::JumpRegister { target } => {
                indirect_branch(code, INDIRECT_JUMP_EXTENSION, target);
            }
            // `call` pushes the return address, which `ret` pops: nothing to
            // save, and a jump past nothing is a plain jump.
            Operation::EnterCalled => {}
            Operation::JumpIntoCalled { target } => {
                indirect_branch(code, INDIRECT_JUMP_EXTENSION, target);
            }
            Operation::Push { source } => opcode_register(code, PUSH_REGISTER_BASE, source),
            Operation::Pop { target } => opcode_register(code, POP_REGISTER_BASE, target),
            Operation::AddStack { amount } => {
                // `lea` rather than `add`, which would clobber the flags the
                // contract says this operation keeps.
                let stack = Memory {
                    base: RSP,
                    displacement: amount,
                };
                register_memory(
                    code,
                    true,
                    false,
                    &LOAD_EFFECTIVE_ADDRESS,
                    number(RSP),
                    stack,
                );
            }
            Operation::LoadCodeAddress { target } => {
                // `lea target, [rip + 0]`: the displacement counts from the
                // end of this instruction, so 0 names the next one.
                let target = number(target);
                rex(code, true, target, 0, false);
                code.extend_from_slice(&LOAD_EFFECTIVE_ADDRESS);
                code.push(modrm(MODRM_MODE_NO_DISPLACEMENT, target, RIP_RELATIVE));
                code.extend_from_slice(&0i32.to_le_bytes());
            }
            Operation::Return => code.push(RETURN_NEAR),
            Operation::Trap => code.extend_from_slice(&UNDEFINED_INSTRUCTION),
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
                let Ok(relative) = i8::try_from(distance - SHORT_BRANCH_SIZE) else {
                    return false;
                };
                code.push(match kind {
                    BranchKind::Unconditional => JUMP_SHORT,
                    BranchKind::Conditional(condition) => {
                        JUMP_SHORT_IF_BASE + condition_code(condition)
                    }
                });
                code.extend_from_slice(&relative.to_le_bytes());
            }
            BranchForm::Long => {
                let size = match kind {
                    BranchKind::Unconditional => JUMP_NEAR_SIZE,
                    BranchKind::Conditional(_) => JUMP_NEAR_IF_SIZE,
                };
                let Ok(relative) = i32::try_from(distance - size) else {
                    return false;
                };
                match kind {
                    BranchKind::Unconditional => code.push(JUMP_NEAR),
                    BranchKind::Conditional(condition) => {
                        code.extend_from_slice(&[
                            JUMP_NEAR_IF_ESCAPE,
                            JUMP_NEAR_IF_BASE + condition_code(condition),
                        ]);
                    }
                }
                code.extend_from_slice(&relative.to_le_bytes());
            }
        }
        true
    }

    fn encode_label_reference(
        &self,
        reference: LabelReference,
        distance: i64,
        code: &mut Vec<u8>,
    ) -> bool {
        // `mov target, [rip + relative]` or `lea target, [rip + relative]`:
        // the displacement counts from the end of the instruction.
        let Ok(relative) = i32::try_from(distance - LABEL_REFERENCE_SIZE) else {
            return false;
        };
        let (opcode, target): (&[u8], Register) = match reference {
            LabelReference::Load { target } => (&MOVE_RM_TO_REGISTER, target),
            LabelReference::Address { target } => (&LOAD_EFFECTIVE_ADDRESS, target),
        };
        let target = number(target);
        rex(code, true, target, 0, false);
        code.extend_from_slice(opcode);
        code.push(modrm(MODRM_MODE_NO_DISPLACEMENT, target, RIP_RELATIVE));
        code.extend_from_slice(&relative.to_le_bytes());
        true
    }
}
