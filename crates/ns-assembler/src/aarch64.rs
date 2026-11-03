//! The AArch64 encoding of the operations.
//!
//! Every instruction is one 32-bit word, so an operation whose operand does
//! not fit the instruction's field takes several: an immediate beyond
//! `imm12`, a displacement beyond both of a load's offset forms, a
//! constant of more than one halfword. What does not fit is materialized
//! into a scratch register the encoder keeps for itself, with `movz`,
//! `movn` and `movk`, which keep the flags, so an operation that keeps
//! them never sets them on the way.
//!
//! Register 31 is the stack pointer in this crate's numbering ([`SP`]),
//! and the zero register in many encodings. The encoder decides per
//! instruction which the field means, never names the zero register as an
//! operand, and moves the stack pointer through [`SCRATCH`] where the form
//! it needs would read the zero register.
//!
//! A call is `blr`, which leaves the return address in `x30`; every
//! address a call reaches begins by pushing it into a 16-byte slot on
//! `sp` (`EnterCalled`), and a return pops it before `ret`, so `sp` stays
//! 16-byte aligned and the return address lies at `[sp + 0]`, where the
//! operations expect it. A conditional branch is one `b.cond` in both forms, reaching a
//! mebibyte either way; past that the code does not assemble, rather than
//! every forward check becoming a taken branch over an unconditional `b`.
//!
//! Every opcode, field shift, field mask, condition code and limit is a
//! named constant whose documentation cites the instruction's page of the
//! Arm A64 Instruction Set Architecture on developer.arm.com, and the
//! registers cite the Procedure Call Standard there. No numeric pattern
//! appears in a function body; `tests/encoding_sources.rs` holds every
//! encoder file to that.

use crate::{
    BranchForm, BranchKind, Condition, Encoder, LabelReference, Memory, Operation, Register, Width,
};

/// `x0`: register number 0, as an instruction's register fields name it.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const X0: Register = Register(0);
/// `x1`: register number 1.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const X1: Register = Register(1);
/// `x2`: register number 2.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const X2: Register = Register(2);
/// `x3`: register number 3.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const X3: Register = Register(3);
/// `x4`: register number 4.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const X4: Register = Register(4);
/// `x5`: register number 5.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const X5: Register = Register(5);
/// `x6`: register number 6.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const X6: Register = Register(6);
/// `x7`: register number 7.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const X7: Register = Register(7);
/// `x8`: register number 8.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const X8: Register = Register(8);
/// `x9`: register number 9.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const X9: Register = Register(9);
/// `x10`: register number 10.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X10: Register = Register(10);
/// `x11`: register number 11.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X11: Register = Register(11);
/// `x12`: register number 12.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X12: Register = Register(12);
/// `x13`: register number 13.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X13: Register = Register(13);
/// `x14`: register number 14.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X14: Register = Register(14);
/// `x15`: register number 15.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X15: Register = Register(15);
/// `x16`: register number 16, IP0, the encoder's first scratch register
/// and never an operand.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers: IP0, the intra-procedure-call scratch register).
pub const X16: Register = Register(16);
/// `x17`: register number 17, IP1, the encoder's second scratch register
/// and never an operand.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers: IP1, the intra-procedure-call scratch register).
pub const X17: Register = Register(17);
/// `x18`: register number 18, the platform register, never an operand on
/// any target.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers: the platform register).
pub const X18: Register = Register(18);
/// `x19`: register number 19.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X19: Register = Register(19);
/// `x20`: register number 20.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X20: Register = Register(20);
/// `x21`: register number 21.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X21: Register = Register(21);
/// `x22`: register number 22.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X22: Register = Register(22);
/// `x23`: register number 23.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X23: Register = Register(23);
/// `x24`: register number 24.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X24: Register = Register(24);
/// `x25`: register number 25.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X25: Register = Register(25);
/// `x26`: register number 26.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X26: Register = Register(26);
/// `x27`: register number 27.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X27: Register = Register(27);
/// `x28`: register number 28.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
pub const X28: Register = Register(28);
/// `x29`: register number 29, the frame pointer of the procedure call
/// standard.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers: the frame pointer).
pub const X29: Register = Register(29);
/// `x30`: register number 30, the link register a call writes, owned by
/// the call convention and never an operand.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers: the link register).
pub const X30: Register = Register(30);
/// `sp`, the stack pointer: register number 31 where an instruction's
/// field reads it as the stack pointer, never the zero register, which no
/// operand names.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers: the stack pointer).
#[expect(
    clippy::min_ident_chars,
    reason = "the architecture's own name of the register"
)]
pub const SP: Register = Register(31);

/// The encoder's first scratch register: the immediate or displacement
/// that does not fit its instruction, or a copy of the stack pointer.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers: IP0 may be used as a temporary register between subroutine
/// calls).
const SCRATCH: Register = X16;

/// The encoder's second scratch register: a stored immediate, a compared
/// word of memory, or an immediate while [`SCRATCH`] holds the stack
/// pointer.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers: IP1 may be used as a temporary register between subroutine
/// calls).
const SECOND_SCRATCH: Register = X17;

/// The registers no operand may be: the two scratch registers, the
/// platform register, which some targets reserve, and the link register,
/// which every call writes.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (General-purpose
/// Registers).
const RESERVED: [Register; 4] = [X16, X17, X18, X30];

/// Register number 31 in a field that reads it as the zero register: the
/// discarded result of `cmp` and `tst`, the stored zero of `str xzr`.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CMP--shifted-register---Compare--shifted-register---an-alias-of-SUBS--shifted-register->
const ZERO_REGISTER: u32 = 31;

/// The A64 encoder. It keeps x16, x17, x18 and x30 out of every operand
/// on every target.
#[derive(Clone, Copy, Default, Debug)]
pub struct Aarch64;

/// The size in bytes of every A64 instruction, and the alignment each
/// must have in memory.
///
/// <https://developer.arm.com/documentation/ddi0487/latest> (A64
/// instructions are 32 bits wide and word-aligned).
const INSTRUCTION_BYTES: u32 = 4;

/// The bytes the stack pointer moves by: it stays 16-byte aligned, so a
/// return address or a saved register takes a slot of 16.
///
/// <https://developer.arm.com/documentation/ihi0055/latest> (The Stack:
/// SP mod 16 = 0).
const STACK_SLOT_BYTES: i32 = 16;

/// The shift that places `Rd` or `Rt`, the destination or the transferred
/// register, bits 4:0.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADD--immediate---Add-immediate-value->
const DESTINATION_SHIFT: u32 = 0;

/// The shift that places `Rn`, the first source or the base, bits 9:5.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADD--immediate---Add-immediate-value->
const FIRST_SOURCE_SHIFT: u32 = 5;

/// The shift that places `Rm`, the second source or the offset register,
/// bits 20:16.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADDS--shifted-register---Add-optionally-shifted-register--setting-flags->
const SECOND_SOURCE_SHIFT: u32 = 16;

/// `MOVZ Xd, #imm16, LSL #shift` (64-bit, `sf` 1) — move a halfword into
/// place, zeroing the other bits.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/MOVZ--Move-wide-with-zero->
const MOVE_WIDE_ZERO: u32 = 0xD280_0000;

/// `MOVN Xd, #imm16, LSL #shift` (64-bit) — move the complement of a
/// halfword in place.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/MOVN--Move-wide-with-NOT->
const MOVE_WIDE_NOT: u32 = 0x9280_0000;

/// `MOVK Xd, #imm16, LSL #shift` (64-bit) — replace one halfword, keeping
/// the others.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/MOVK--Move-wide-with-keep->
const MOVE_WIDE_KEEP: u32 = 0xF280_0000;

/// The shift that places a move-wide instruction's `imm16`, bits 20:5.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/MOVZ--Move-wide-with-zero->
const WIDE_IMMEDIATE_SHIFT: u32 = 5;

/// The shift that places a move-wide instruction's `hw`, which halfword
/// the immediate goes to, bits 22:21.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/MOVZ--Move-wide-with-zero->
const HALFWORD_INDEX_SHIFT: u32 = 21;

/// The bits of one halfword, the move-wide immediate.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/MOVZ--Move-wide-with-zero->
const HALFWORD_BITS: u32 = 16;

/// A halfword's value bits.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/MOVZ--Move-wide-with-zero->
const HALFWORD_MASK: u64 = 0xFFFF;

/// The halfwords of a 64-bit register, the values `hw` takes.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/MOVZ--Move-wide-with-zero->
const HALFWORD_COUNT: u32 = 4;

/// `ORR Xd, XZR, Xm` (shifted register, 64-bit), the `MOV` (register)
/// alias with `Rn` the zero register already in place.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/MOV--register---Move-register-value--an-alias-of-ORR--shifted-register->
const MOVE_REGISTER: u32 = 0xAA00_03E0;

/// `CSEL Xd, Xn, Xm, cond` (64-bit) — `Xn` when the condition holds, `Xm`
/// otherwise.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CSEL--Conditional-select->
const CONDITIONAL_SELECT: u32 = 0x9A80_0000;

/// The shift that places `CSEL`'s `cond`, bits 15:12.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CSEL--Conditional-select->
const CONDITION_SHIFT: u32 = 12;

/// `ADD Xd|SP, Xn|SP, #imm12` (64-bit) — add without setting the flags;
/// both register fields read 31 as the stack pointer. With an immediate of
/// 0, the `MOV` to or from `SP` alias.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADD--immediate---Add-immediate-value->
const ADD_IMMEDIATE: u32 = 0x9100_0000;

/// `SUB Xd|SP, Xn|SP, #imm12` (64-bit) — subtract without setting the
/// flags.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/SUB--immediate---Subtract-immediate-value->
const SUBTRACT_IMMEDIATE: u32 = 0xD100_0000;

/// `ADDS Xd, Xn|SP, #imm12` (64-bit) — add, setting the flags; `Rd` 31 is
/// the zero register, the `CMN` alias.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADDS--immediate---Add-immediate--setting-flags->
const ADD_IMMEDIATE_SETTING_FLAGS: u32 = 0xB100_0000;

/// `SUBS Xd, Xn|SP, #imm12` (64-bit) — subtract, setting the flags; `Rd`
/// 31 is the zero register, the `CMP` alias.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/SUBS--immediate---Subtract-immediate-value--setting-flags->
const SUBTRACT_IMMEDIATE_SETTING_FLAGS: u32 = 0xF100_0000;

/// The shift that places an add or subtract's `imm12`, bits 21:10.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADDS--immediate---Add-immediate--setting-flags->
const ARITHMETIC_IMMEDIATE_SHIFT: u32 = 10;

/// The largest `imm12`.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADDS--immediate---Add-immediate--setting-flags->
const IMMEDIATE_12_MAX: u64 = 0xFFF;

/// The amount `sh` shifts `imm12` left by when set.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADDS--immediate---Add-immediate--setting-flags->
const IMMEDIATE_12_SHIFT_AMOUNT: u32 = 12;

/// `sh`, bit 22: the immediate is shifted left by 12.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADDS--immediate---Add-immediate--setting-flags->
const IMMEDIATE_SHIFTED: u32 = 1 << 22;

/// `ADDS Xd, Xn, Xm` (shifted register, 64-bit, no shift) — add, setting
/// the flags; 31 is the zero register in every field.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADDS--shifted-register---Add-optionally-shifted-register--setting-flags->
const ADD_REGISTER_SETTING_FLAGS: u32 = 0xAB00_0000;

/// `SUBS Xd, Xn, Xm` (shifted register, 64-bit, no shift) — subtract,
/// setting the flags; `Rd` 31 is the `CMP` alias.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/SUBS--shifted-register---Subtract-optionally-shifted-register--setting-flags->
const SUBTRACT_REGISTER_SETTING_FLAGS: u32 = 0xEB00_0000;

/// `ADD Xd|SP, Xn|SP, Xm, UXTX` (extended register, 64-bit, `option` 011,
/// no shift) — add a register to the stack pointer without setting the
/// flags.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADD--extended-register---Add-extended-and-scaled-register->
const ADD_EXTENDED_REGISTER: u32 = 0x8B20_6000;

/// `SUBS XZR, Xn|SP, Xm, UXTX` (extended register, 64-bit) without its
/// `Rd`: the `CMP` whose left operand may be the stack pointer.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/SUBS--extended-register---Subtract-extended-and-scaled-register--setting-flags->
const SUBTRACT_EXTENDED_REGISTER_SETTING_FLAGS: u32 = 0xEB20_6000;

/// `AND Xd, Xn, Xm` (shifted register, 64-bit, no shift).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/AND--shifted-register---Bitwise-AND--shifted-register-->
const AND_REGISTER: u32 = 0x8A00_0000;

/// `ORR Xd, Xn, Xm` (shifted register, 64-bit, no shift).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ORR--shifted-register---Bitwise-OR--shifted-register-->
const OR_REGISTER: u32 = 0xAA00_0000;

/// `ANDS XZR, Xn, Xm` (shifted register, 64-bit) without its registers,
/// the `TST` alias.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ANDS--shifted-register---Bitwise-AND--shifted-register---setting-flags->
const AND_REGISTER_SETTING_FLAGS: u32 = 0xEA00_0000;

/// `ANDS WZR, Wn, Wm` (shifted register, 32-bit), the 32-bit `TST`.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ANDS--shifted-register---Bitwise-AND--shifted-register---setting-flags->
const AND_WORD_REGISTER_SETTING_FLAGS: u32 = 0x6A00_0000;

/// `AND Xd|SP, Xn, #bitmask` (immediate, 64-bit); `Rd` 31 is the stack
/// pointer, `Rn` 31 the zero register.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/AND--immediate---Bitwise-AND--immediate-->
const AND_IMMEDIATE: u32 = 0x9200_0000;

/// `ORR Xd|SP, Xn, #bitmask` (immediate, 64-bit).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ORR--immediate---Bitwise-OR--immediate-->
const OR_IMMEDIATE: u32 = 0xB200_0000;

/// `ANDS XZR, Xn, #bitmask` (immediate, 64-bit), the `TST` alias.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ANDS--immediate---Bitwise-AND--immediate---setting-flags->
const AND_IMMEDIATE_SETTING_FLAGS: u32 = 0xF200_0000;

/// `ANDS WZR, Wn, #bitmask` (immediate, 32-bit), whose bitmask has no `N`
/// bit and elements of at most 32 bits.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ANDS--immediate---Bitwise-AND--immediate---setting-flags->
const AND_WORD_IMMEDIATE_SETTING_FLAGS: u32 = 0x7200_0000;

/// The shift that places a logical immediate's `N:immr:imms`, bits 22:10.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/AND--immediate---Bitwise-AND--immediate-->
const LOGICAL_IMMEDIATE_SHIFT: u32 = 10;

/// The position of `N` within `N:immr:imms`: set for an element of 64
/// bits.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/AND--immediate---Bitwise-AND--immediate-->
const LOGICAL_ELEMENT_IS_REGISTER_SHIFT: u32 = 12;

/// The position of `immr` within `N:immr:imms`: the right rotation of the
/// element's run of ones.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/AND--immediate---Bitwise-AND--immediate-->
const LOGICAL_ROTATION_SHIFT: u32 = 6;

/// The six bits of `imms`, which hold the element size's prefix of ones and
/// the run's length minus one.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/AND--immediate---Bitwise-AND--immediate-->
const LOGICAL_SIZE_MASK: u32 = 0x3F;

/// The bits of a register, the largest element of a bitmask.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/AND--immediate---Bitwise-AND--immediate-->
const REGISTER_BITS: u32 = 64;

/// The bits of a 32-bit register: a `W` form's bitmask repeats every 32
/// bits or less.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ANDS--immediate---Bitwise-AND--immediate---setting-flags->
const WORD_BITS: u32 = 32;

/// The smallest element of a bitmask.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/AND--immediate---Bitwise-AND--immediate-->
const SMALLEST_ELEMENT_BITS: u32 = 2;

/// `UBFM Xd, Xn, #immr, #imms` (64-bit, `N` 1) — the `LSL` and `LSR`
/// aliases.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/UBFM--Unsigned-bitfield-move->
const UNSIGNED_BITFIELD_MOVE: u32 = 0xD340_0000;

/// `SBFM Xd, Xn, #immr, #imms` (64-bit, `N` 1) — the `ASR` alias.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/SBFM--Signed-bitfield-move->
const SIGNED_BITFIELD_MOVE: u32 = 0x9340_0000;

/// The shift that places a bitfield move's `immr`, bits 21:16.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/UBFM--Unsigned-bitfield-move->
const BITFIELD_ROTATION_SHIFT: u32 = 16;

/// The shift that places a bitfield move's `imms`, bits 15:10.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/UBFM--Unsigned-bitfield-move->
const BITFIELD_TOP_SHIFT: u32 = 10;

/// The highest bit of a 64-bit register, the `imms` of a right shift.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LSR--immediate---Logical-shift-right--immediate---an-alias-of-UBFM->
const HIGHEST_BIT: u32 = 63;

/// The widest 64-bit shift count plus one.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LSL--immediate---Logical-shift-left--immediate---an-alias-of-UBFM->
const SHIFT_COUNT_LIMIT: u8 = 64;

/// `STRB Wt, [Xn|SP, #imm12]` (unsigned offset), the store of the
/// unsigned-offset family with `size` and `opc` zero; `size` and
/// [`LOAD`] make the others.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/STRB--immediate---Store-register-byte--immediate-->
const UNSIGNED_OFFSET_ACCESS: u32 = 0x3900_0000;

/// `STURB Wt, [Xn|SP, #simm9]` (unscaled), the unscaled family's byte
/// store.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/STURB--Store-register-byte--unscaled-->
const UNSCALED_ACCESS: u32 = 0x3800_0000;

/// `STRB Wt, [Xn|SP, Xm]` (register offset, `option` 011, `S` 0), the
/// register-offset family's byte store.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/STRB--register---Store-register-byte--register-->
const REGISTER_OFFSET_ACCESS: u32 = 0x3820_6800;

/// `opc` 01, bit 22: the load of the store `opc` 00 names, zero-extending.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LDR--immediate---Load-register--immediate-->
const LOAD: u32 = 1 << 22;

/// The shift that places a load or store's `size`, the log2 of its bytes,
/// bits 31:30.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LDR--immediate---Load-register--immediate-->
const ACCESS_SIZE_SHIFT: u32 = 30;

/// `size` 00: a byte.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LDRB--immediate---Load-register-byte--immediate-->
const SIZE_BYTE: u32 = 0b00;

/// `size` 01: a halfword.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LDRH--immediate---Load-register-halfword--immediate-->
const SIZE_HALF: u32 = 0b01;

/// `size` 10: a 32-bit word.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LDR--immediate---Load-register--immediate-->
const SIZE_WORD: u32 = 0b10;

/// `size` 11: a 64-bit word.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LDR--immediate---Load-register--immediate-->
const SIZE_DOUBLEWORD: u32 = 0b11;

/// The shift that places the unsigned offset's `imm12`, in units of the
/// access's size, bits 21:10.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LDR--immediate---Load-register--immediate-->
const OFFSET_SHIFT: u32 = 10;

/// The shift that places the unscaled offset's `imm9`, bits 20:12.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LDUR--Load-register--unscaled-->
const UNSCALED_OFFSET_SHIFT: u32 = 12;

/// The bits of `imm9`, a two's-complement byte offset.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LDUR--Load-register--unscaled-->
const UNSCALED_OFFSET_MASK: u64 = 0x1FF;

/// The smallest unscaled offset.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LDUR--Load-register--unscaled-->
const UNSCALED_OFFSET_MIN: i64 = -256;

/// The largest unscaled offset.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LDUR--Load-register--unscaled-->
const UNSCALED_OFFSET_MAX: i64 = 255;

/// The condition code `eq` (Z set).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CSEL--Conditional-select->
const CONDITION_EQUAL: u32 = 0b0000;

/// The condition code `ne` (Z clear).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CSEL--Conditional-select->
const CONDITION_NOT_EQUAL: u32 = 0b0001;

/// The condition code `hs` (C set: no borrow, unsigned higher or same).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CSEL--Conditional-select->
const CONDITION_HIGHER_OR_SAME: u32 = 0b0010;

/// The condition code `lo` (C clear: unsigned lower).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CSEL--Conditional-select->
const CONDITION_LOWER: u32 = 0b0011;

/// The condition code `vs` (V set: signed overflow).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CSEL--Conditional-select->
const CONDITION_OVERFLOW: u32 = 0b0110;

/// The condition code `vc` (V clear).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CSEL--Conditional-select->
const CONDITION_NO_OVERFLOW: u32 = 0b0111;

/// The condition code `hi` (C set and Z clear: unsigned higher).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CSEL--Conditional-select->
const CONDITION_HIGHER: u32 = 0b1000;

/// The condition code `ls` (C clear or Z set: unsigned lower or same).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CSEL--Conditional-select->
const CONDITION_LOWER_OR_SAME: u32 = 0b1001;

/// The condition code `ge` (N equals V: signed greater or equal).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CSEL--Conditional-select->
const CONDITION_GREATER_OR_EQUAL: u32 = 0b1010;

/// The condition code `lt` (N differs from V: signed less).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CSEL--Conditional-select->
const CONDITION_LESS: u32 = 0b1011;

/// The condition code `gt` (Z clear and N equals V: signed greater).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CSEL--Conditional-select->
const CONDITION_GREATER: u32 = 0b1100;

/// The condition code `le` (Z set or N differs from V: signed less or
/// equal).
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CSEL--Conditional-select->
const CONDITION_LESS_OR_EQUAL: u32 = 0b1101;

/// `BLR Xn` — call the address in `Xn`, the return address into `x30`.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/BLR--Branch-with-link-to-register->
const BRANCH_WITH_LINK_REGISTER: u32 = 0xD63F_0000;

/// `BR Xn` — jump to the address in `Xn`.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/BR--Branch-to-register->
const BRANCH_REGISTER: u32 = 0xD61F_0000;

/// `RET` — return to the address in `x30`, the default `Rn` already in
/// place.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/RET--Return-from-subroutine->
const RETURN_TO_LINK_REGISTER: u32 = 0xD65F_03C0;

/// `STR Xt, [Xn|SP, #simm9]!` (immediate, pre-index, 64-bit) — store, then
/// keep the lowered address in the base: a push.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/STR--immediate---Store-register--immediate-->
const STORE_PRE_INDEX: u32 = 0xF800_0C00;

/// `LDR Xt, [Xn|SP], #simm9` (immediate, post-index, 64-bit) — load, then
/// raise the base: a pop.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LDR--immediate---Load-register--immediate-->
const LOAD_POST_INDEX: u32 = 0xF840_0400;

/// `SMULH Xd, Xn, Xm` (`Ra` 31 in place) — the high 64 bits of the signed
/// 128-bit product.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/SMULH--Signed-multiply-high->
const SIGNED_MULTIPLY_HIGH: u32 = 0x9B40_7C00;

/// `MUL Xd, Xn, Xm`, the `MADD` alias with the zero register as `Ra`
/// already in place — the low 64 bits of the product.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/MUL--Multiply--an-alias-of-MADD->
const MULTIPLY: u32 = 0x9B00_7C00;

/// `MSUB Xd, Xn, Xm, Xa` (64-bit) — `Xa - Xn * Xm`.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/MSUB--Multiply-subtract->
const MULTIPLY_SUBTRACT: u32 = 0x9B00_8000;

/// The shift that places a multiply-add's `Ra`, the addend, bits 14:10.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/MSUB--Multiply-subtract->
const ADDEND_SHIFT: u32 = 10;

/// `SDIV Xd, Xn, Xm` (64-bit) — the signed quotient, truncated toward
/// zero.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/SDIV--Signed-divide->
const SIGNED_DIVIDE: u32 = 0x9AC0_0C00;

/// `shift` 10, bits 23:22, of a shifted-register `SUBS`: the second
/// operand shifted right arithmetically.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/SUBS--shifted-register---Subtract-optionally-shifted-register--setting-flags->
const SHIFTED_ARITHMETIC_RIGHT: u32 = 0b10 << 22;

/// The shift that places a shifted-register operand's `imm6`, its shift
/// amount, bits 15:10.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/SUBS--shifted-register---Subtract-optionally-shifted-register--setting-flags->
const SHIFT_AMOUNT_SHIFT: u32 = 10;

/// `CCMP Xn, Xm, #nzcv, cond` (register, 64-bit) — the flags of
/// `Xn - Xm` when `cond` holds, `nzcv` otherwise.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CCMP--register---Conditional-compare--register-->
const CONDITIONAL_COMPARE_REGISTER: u32 = 0xFA40_0000;

/// The `nzcv` of a conditional compare with V alone set, bits 3:0.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/CCMP--register---Conditional-compare--register-->
const FLAGS_OVERFLOW_ALONE: u32 = 0b0001;

/// `ADR Xd, label` — the address `immhi:immlo` bytes from the
/// instruction.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADR--Form-PC-relative-address->
const ADDRESS: u32 = 0x1000_0000;

/// The bits of `ADR`'s `immlo`, the distance's low two bits, at bits
/// 30:29.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADR--Form-PC-relative-address->
const ADDRESS_LOW_BITS: u32 = 2;

/// The shift that places `ADR`'s `immlo`.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADR--Form-PC-relative-address->
const ADDRESS_LOW_SHIFT: u32 = 29;

/// The bits of `ADR`'s signed distance, `immhi:immlo`.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/ADR--Form-PC-relative-address->
const ADDRESS_DISTANCE_BITS: u32 = 21;

/// `LDR Xt, label` (literal, 64-bit) — load the word `imm19` instructions
/// from the instruction.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/LDR--literal---Load-register--literal-->
const LOAD_LITERAL: u32 = 0x5800_0000;

/// `B label` — jump `imm26` instructions from the instruction.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/B--Branch->
const BRANCH: u32 = 0x1400_0000;

/// The bits of `B`'s signed `imm26`, in instructions.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/B--Branch->
const BRANCH_DISTANCE_BITS: u32 = 26;

/// `B.cond label` — jump `imm19` instructions from the instruction when
/// `cond`, at bits 3:0, holds.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/B-cond--Branch-conditionally->
const BRANCH_CONDITIONAL: u32 = 0x5400_0000;

/// The bits of the signed `imm19` of `B.cond` and `LDR` (literal), in
/// instructions.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/B-cond--Branch-conditionally->
const CONDITIONAL_DISTANCE_BITS: u32 = 19;

/// The shift that places the `imm19` of `B.cond` and `LDR` (literal), and
/// `ADR`'s `immhi`, bits 23:5.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/B-cond--Branch-conditionally->
const CONDITIONAL_DISTANCE_SHIFT: u32 = 5;

/// `UDF #0` — permanently undefined: the stop, and the trap.
///
/// <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/UDF--Permanently-undefined->
const UNDEFINED: u32 = 0x0000_0000;

/// A register's number for a field that reads 31 as the stack pointer:
/// any register but the reserved ones.
fn general_or_stack(register: Register) -> u32 {
    assert!(
        register.0 <= SP.0,
        "AArch64 has no register {}; registers are numbered 0 to 31, 31 the stack pointer",
        register.0
    );
    assert!(
        !RESERVED.contains(&register),
        "x{} is reserved for the encoder and the call convention",
        register.0
    );
    u32::from(register.0)
}

/// A register's number for a field that reads 31 as the zero register,
/// where the stack pointer cannot stand.
fn general(register: Register, operation: &str) -> u32 {
    let number = general_or_stack(register);
    assert!(
        register != SP,
        "the stack pointer is no operand of {operation}"
    );
    number
}

/// The number of a register the encoder names itself: a scratch register
/// or the stack pointer, which no check refuses.
fn own(register: Register) -> u32 {
    u32::from(register.0)
}

fn emit(code: &mut Vec<u8>, instruction: u32) {
    code.extend_from_slice(&instruction.to_le_bytes());
}

/// `base` with its destination, first and second source fields.
fn three_registers(base: u32, destination: u32, first: u32, second: u32) -> u32 {
    base | (destination << DESTINATION_SHIFT)
        | (first << FIRST_SOURCE_SHIFT)
        | (second << SECOND_SOURCE_SHIFT)
}

/// The bits of `value` under `mask`, a mask of at most 32 bits.
fn low_bits(value: u64, mask: u64) -> u32 {
    match u32::try_from(value & mask) {
        Ok(bits) => bits,
        Err(_) => panic!("a field mask has at most 32 bits"),
    }
}

/// `target := value`, by the shorter of a `movz` and a `movn` sequence,
/// each followed by a `movk` per halfword left to set: none of them touches
/// the flags.
fn move_immediate(code: &mut Vec<u8>, target: u32, value: u64) {
    let halfword = |index: u32| low_bits(value >> (index * HALFWORD_BITS), HALFWORD_MASK);
    let all_ones = low_bits(HALFWORD_MASK, HALFWORD_MASK);
    let differing = |skipped: u32| {
        (0..HALFWORD_COUNT)
            .filter(|index| halfword(*index) != skipped)
            .count()
    };
    let inverted = differing(all_ones) < differing(0);
    let skipped = if inverted { all_ones } else { 0 };
    let first = (0..HALFWORD_COUNT)
        .find(|index| halfword(*index) != skipped)
        .unwrap_or(0);
    let (opcode, immediate) = if inverted {
        (MOVE_WIDE_NOT, !halfword(first) & all_ones)
    } else {
        (MOVE_WIDE_ZERO, halfword(first))
    };
    let move_wide = |opcode: u32, index: u32, immediate: u32| {
        opcode
            | (index << HALFWORD_INDEX_SHIFT)
            | (immediate << WIDE_IMMEDIATE_SHIFT)
            | (target << DESTINATION_SHIFT)
    };
    emit(code, move_wide(opcode, first, immediate));
    for index in first + 1..HALFWORD_COUNT {
        if halfword(index) != skipped {
            emit(code, move_wide(MOVE_WIDE_KEEP, index, halfword(index)));
        }
    }
}

/// `target := source` where either may be the stack pointer: `add #0`,
/// which keeps the flags.
fn move_through_add(code: &mut Vec<u8>, target: u32, source: u32) {
    emit(code, three_registers(ADD_IMMEDIATE, target, source, 0));
}

/// The `imm12` and `sh` bits of an add or subtract of `magnitude`, when
/// one of the two forms holds it.
fn arithmetic_immediate(magnitude: u64) -> Option<u32> {
    if magnitude <= IMMEDIATE_12_MAX {
        Some(low_bits(magnitude, IMMEDIATE_12_MAX) << ARITHMETIC_IMMEDIATE_SHIFT)
    } else if magnitude & IMMEDIATE_12_MAX == 0
        && magnitude >> IMMEDIATE_12_SHIFT_AMOUNT <= IMMEDIATE_12_MAX
    {
        let shifted = low_bits(magnitude >> IMMEDIATE_12_SHIFT_AMOUNT, IMMEDIATE_12_MAX);
        Some((shifted << ARITHMETIC_IMMEDIATE_SHIFT) | IMMEDIATE_SHIFTED)
    } else {
        None
    }
}

/// An add of `value` sign-extended, `add` for a non-negative one and
/// `subtract` of its magnitude for a negative one, when an immediate form
/// holds it: the instruction without its registers.
fn signed_arithmetic(value: i32, add: u32, subtract: u32) -> Option<u32> {
    let magnitude = i64::from(value).unsigned_abs();
    let opcode = if value < 0 { subtract } else { add };
    arithmetic_immediate(magnitude).map(|immediate| opcode | immediate)
}

fn sign_extended(value: i32) -> u64 {
    i64::from(value).cast_unsigned()
}

/// The bitmask immediate of an AND, ORR, EOR or ANDS of 64 bits: its
/// `N:immr:imms`, `N` at bit 12, `immr` at bits 11:6 and `imms` at bits
/// 5:0, or `None` when `value` is no element of 2 to 64 bits, a rotated
/// run of ones neither empty nor full, repeated over the register.
#[must_use]
pub fn logical_immediate(value: u64) -> Option<u32> {
    if value == 0 || value == u64::MAX {
        return None;
    }
    // The element is the smallest repeating unit.
    let mut size = REGISTER_BITS;
    while size > SMALLEST_ELEMENT_BITS {
        let half = size >> 1;
        let mask = element_mask(half);
        if value & mask != (value >> half) & mask {
            break;
        }
        size = half;
    }
    let element = value & element_mask(size);
    let ones = element.count_ones();
    let run = element_mask(ones);
    let rotation = (0..size).find(|rotation| rotate_right(run, *rotation, size) == element)?;
    let element_is_register = u32::from(size == REGISTER_BITS);
    let size_prefix = (!(size - 1) << 1) & LOGICAL_SIZE_MASK;
    Some(
        (element_is_register << LOGICAL_ELEMENT_IS_REGISTER_SHIFT)
            | (rotation << LOGICAL_ROTATION_SHIFT)
            | size_prefix
            | (ones - 1),
    )
}

/// The low `bits` bits set.
fn element_mask(bits: u32) -> u64 {
    if bits >= REGISTER_BITS {
        u64::MAX
    } else {
        (1 << bits) - 1
    }
}

/// `value`, an element of `size` bits, rotated right by `amount` within
/// it.
fn rotate_right(value: u64, amount: u32, size: u32) -> u64 {
    if amount == 0 {
        value
    } else {
        ((value >> amount) | (value << (size - amount))) & element_mask(size)
    }
}

/// `target := target op value` sign-extended, `op` an AND or an ORR: the
/// bitmask form when `value` has one, else `value` through a scratch
/// register. The immediate form writes the stack pointer but reads the
/// zero register, so a stack pointer target is first copied to
/// [`SCRATCH`].
fn logical(
    code: &mut Vec<u8>,
    immediate_opcode: u32,
    register_opcode: u32,
    target: Register,
    value: i32,
) {
    let value = sign_extended(value);
    let bitmask = logical_immediate(value);
    let scratch = own(SCRATCH);
    let target_number = general_or_stack(target);
    if target == SP {
        let stack = own(SP);
        move_through_add(code, scratch, stack);
        if let Some(bitmask) = bitmask {
            emit(
                code,
                three_registers(immediate_opcode, stack, scratch, 0)
                    | (bitmask << LOGICAL_IMMEDIATE_SHIFT),
            );
        } else {
            let second = own(SECOND_SCRATCH);
            move_immediate(code, second, value);
            emit(
                code,
                three_registers(register_opcode, scratch, scratch, second),
            );
            move_through_add(code, stack, scratch);
        }
    } else if let Some(bitmask) = bitmask {
        emit(
            code,
            three_registers(immediate_opcode, target_number, target_number, 0)
                | (bitmask << LOGICAL_IMMEDIATE_SHIFT),
        );
    } else {
        move_immediate(code, scratch, value);
        emit(
            code,
            three_registers(register_opcode, target_number, target_number, scratch),
        );
    }
}

/// A load (`load`) or a store of `register`'s `width` bits at `memory`:
/// the scaled unsigned offset when the displacement is a multiple of the
/// width below 4096 of them, the unscaled one within -256 to 255, else the
/// displacement into [`SCRATCH`] and the register offset.
fn access(code: &mut Vec<u8>, width: Width, load: bool, register: u32, memory: Memory) {
    let base = general_or_stack(memory.base);
    let size = match width {
        Width::Byte => SIZE_BYTE,
        Width::Half => SIZE_HALF,
        Width::Word32 => SIZE_WORD,
        Width::Word64 => SIZE_DOUBLEWORD,
    };
    let kind = (size << ACCESS_SIZE_SHIFT) | if load { LOAD } else { 0 };
    let displacement = i64::from(memory.displacement);
    let scale = 1_i64 << size;
    let scaled = displacement / scale;
    if displacement >= 0 && displacement % scale == 0 && scaled.cast_unsigned() <= IMMEDIATE_12_MAX
    {
        let offset = low_bits(scaled.cast_unsigned(), IMMEDIATE_12_MAX);
        emit(
            code,
            three_registers(UNSIGNED_OFFSET_ACCESS | kind, register, base, 0)
                | (offset << OFFSET_SHIFT),
        );
    } else if (UNSCALED_OFFSET_MIN..=UNSCALED_OFFSET_MAX).contains(&displacement) {
        let offset = low_bits(displacement.cast_unsigned(), UNSCALED_OFFSET_MASK);
        emit(
            code,
            three_registers(UNSCALED_ACCESS | kind, register, base, 0)
                | (offset << UNSCALED_OFFSET_SHIFT),
        );
    } else {
        let scratch = own(SCRATCH);
        move_immediate(code, scratch, displacement.cast_unsigned());
        emit(
            code,
            three_registers(REGISTER_OFFSET_ACCESS | kind, register, base, scratch),
        );
    }
}

/// The flags of `left - right`, `right` no stack pointer: the extended
/// register form when `left` is one, which reads 31 there as the stack
/// pointer.
fn compare_registers(code: &mut Vec<u8>, left: u32, right: u32) {
    let opcode = if left == own(SP) {
        SUBTRACT_EXTENDED_REGISTER_SETTING_FLAGS
    } else {
        SUBTRACT_REGISTER_SETTING_FLAGS
    };
    emit(code, three_registers(opcode, ZERO_REGISTER, left, right));
}

/// The flags of `left & right` over `width`, `Equal` when zero: `tst`
/// with the bitmask when `right` has one, else with `right` in a scratch
/// register. `tst` reads 31 as the zero register, so a stack pointer is
/// first copied to [`SCRATCH`].
fn test_immediate(code: &mut Vec<u8>, width: Width, left: Register, right: i32) {
    let (immediate_opcode, register_opcode, bitmask) = match width {
        Width::Word64 => (
            AND_IMMEDIATE_SETTING_FLAGS,
            AND_REGISTER_SETTING_FLAGS,
            logical_immediate(sign_extended(right)),
        ),
        Width::Word32 => {
            // A 32-bit bitmask is a 64-bit one whose element repeats in
            // each half, which leaves `N` clear.
            let word = u64::from(right.cast_unsigned());
            (
                AND_WORD_IMMEDIATE_SETTING_FLAGS,
                AND_WORD_REGISTER_SETTING_FLAGS,
                logical_immediate(word | (word << WORD_BITS)),
            )
        }
        Width::Byte | Width::Half => panic!("a test is of 32 or 64 bits"),
    };
    let mut left = general_or_stack(left);
    let mut scratch = own(SCRATCH);
    if left == own(SP) {
        move_through_add(code, scratch, left);
        left = scratch;
        scratch = own(SECOND_SCRATCH);
    }
    if let Some(bitmask) = bitmask {
        emit(
            code,
            three_registers(immediate_opcode, ZERO_REGISTER, left, 0)
                | (bitmask << LOGICAL_IMMEDIATE_SHIFT),
        );
    } else {
        // Only the low 32 bits matter to the `W` form, and the
        // sign-extended value takes at most two instructions.
        move_immediate(code, scratch, sign_extended(right));
        emit(
            code,
            three_registers(register_opcode, ZERO_REGISTER, left, scratch),
        );
    }
}

/// A shift by a constant, the bitfield move `opcode` with `immr` and
/// `imms`.
fn bitfield_move(code: &mut Vec<u8>, opcode: u32, target: Register, rotation: u32, top: u32) {
    let target = general(target, "a shift");
    emit(
        code,
        three_registers(opcode, target, target, 0)
            | (rotation << BITFIELD_ROTATION_SHIFT)
            | (top << BITFIELD_TOP_SHIFT),
    );
}

fn shift_count(count: u8) -> u32 {
    assert!(
        count < SHIFT_COUNT_LIMIT,
        "a shift by {count} is no 64-bit shift"
    );
    u32::from(count)
}

/// `distance` as a signed field of `bits` bits counting units of `unit`
/// bytes, when it is a whole number of them the field holds.
fn signed_field(distance: i64, unit: i64, bits: u32) -> Option<u32> {
    if distance % unit != 0 {
        return None;
    }
    let units = distance / unit;
    let limit = 1_i64 << (bits - 1);
    if units < -limit || units >= limit {
        return None;
    }
    Some(low_bits(units.cast_unsigned(), element_mask(bits)))
}

/// `distance` in instructions, as a signed field of `bits` bits.
fn instruction_field(distance: i64, bits: u32) -> Option<u32> {
    signed_field(distance, i64::from(INSTRUCTION_BYTES), bits)
}

/// `adr target` of the address `distance` bytes away, which reaches every
/// byte within a mebibyte.
fn address(target: u32, distance: i64) -> Option<u32> {
    let field = signed_field(distance, 1, ADDRESS_DISTANCE_BITS)?;
    let low = field & ((1 << ADDRESS_LOW_BITS) - 1);
    let high = field >> ADDRESS_LOW_BITS;
    Some(
        ADDRESS
            | (low << ADDRESS_LOW_SHIFT)
            | (high << CONDITIONAL_DISTANCE_SHIFT)
            | (target << DESTINATION_SHIFT),
    )
}

/// `str source, [sp, #-16]!`: one slot down, keeping `sp` 16-byte aligned.
fn push(code: &mut Vec<u8>, source: u32) {
    let offset = low_bits(sign_extended(-STACK_SLOT_BYTES), UNSCALED_OFFSET_MASK);
    emit(
        code,
        three_registers(STORE_PRE_INDEX, source, own(SP), 0) | (offset << UNSCALED_OFFSET_SHIFT),
    );
}

/// `ldr target, [sp], #16`: the slot [`push`] filled.
fn pop(code: &mut Vec<u8>, target: u32) {
    let offset = low_bits(sign_extended(STACK_SLOT_BYTES), UNSCALED_OFFSET_MASK);
    emit(
        code,
        three_registers(LOAD_POST_INDEX, target, own(SP), 0) | (offset << UNSCALED_OFFSET_SHIFT),
    );
}

/// The condition code `cond`.
fn condition_code(condition: Condition) -> u32 {
    match condition {
        Condition::Equal => CONDITION_EQUAL,
        Condition::NotEqual => CONDITION_NOT_EQUAL,
        Condition::AboveOrEqual => CONDITION_HIGHER_OR_SAME,
        Condition::Below => CONDITION_LOWER,
        Condition::Overflow => CONDITION_OVERFLOW,
        Condition::NoOverflow => CONDITION_NO_OVERFLOW,
        Condition::Above => CONDITION_HIGHER,
        Condition::BelowOrEqual => CONDITION_LOWER_OR_SAME,
        Condition::GreaterOrEqual => CONDITION_GREATER_OR_EQUAL,
        Condition::Less => CONDITION_LESS,
        Condition::Greater => CONDITION_GREATER,
        Condition::LessOrEqual => CONDITION_LESS_OR_EQUAL,
    }
}

impl Encoder for Aarch64 {
    fn stack_slot_bytes(&self) -> i32 {
        STACK_SLOT_BYTES
    }

    fn instruction_alignment(&self) -> u32 {
        INSTRUCTION_BYTES
    }

    fn fill_with_stops(&self, length: usize, code: &mut Vec<u8>) {
        let instruction_bytes = INSTRUCTION_BYTES as usize;
        // A part of an instruction would leave the code after the fill
        // misaligned, and no instruction runs from there.
        assert!(
            length.is_multiple_of(instruction_bytes),
            "a stop fill is a multiple of 4 bytes, not {length}"
        );
        for _ in 0..length / instruction_bytes {
            emit(code, UNDEFINED);
        }
    }

    fn encode(&self, operation: Operation, code: &mut Vec<u8>) {
        match operation {
            Operation::MoveRegister { target, source } => {
                let (target, source) = (general_or_stack(target), general_or_stack(source));
                let stack = own(SP);
                // `orr` reads 31 as the zero register, so a move to or from
                // the stack pointer is an `add #0`.
                if target == stack || source == stack {
                    move_through_add(code, target, source);
                } else {
                    emit(code, three_registers(MOVE_REGISTER, target, 0, source));
                }
            }
            Operation::MoveIf {
                condition,
                target,
                source,
            } => {
                let (target, source) = (general(target, "a csel"), general(source, "a csel"));
                emit(
                    code,
                    three_registers(CONDITIONAL_SELECT, target, source, target)
                        | (condition_code(condition) << CONDITION_SHIFT),
                );
            }
            Operation::MoveImmediate { target, value } => {
                move_immediate(code, general(target, "a move of an immediate"), value);
            }
            Operation::Load {
                width,
                target,
                source,
            } => access(code, width, true, general(target, "a load"), source),
            Operation::Store {
                width,
                target,
                source,
            } => access(code, width, false, general(source, "a store"), target),
            Operation::StoreImmediate32 { target, value } => {
                if value == 0 {
                    access(code, Width::Word64, false, ZERO_REGISTER, target);
                } else {
                    // The second scratch register, since the first may
                    // carry the displacement.
                    let second = own(SECOND_SCRATCH);
                    move_immediate(code, second, sign_extended(value));
                    access(code, Width::Word64, false, second, target);
                }
            }
            Operation::Add { target, source } => {
                let (target, source) = (general(target, "an add"), general(source, "an add"));
                emit(
                    code,
                    three_registers(ADD_REGISTER_SETTING_FLAGS, target, target, source),
                );
            }
            Operation::AddImmediate { target, value } => {
                let target = general(target, "an add");
                if let Some(opcode) = signed_arithmetic(
                    value,
                    ADD_IMMEDIATE_SETTING_FLAGS,
                    SUBTRACT_IMMEDIATE_SETTING_FLAGS,
                ) {
                    emit(code, three_registers(opcode, target, target, 0));
                } else {
                    let scratch = own(SCRATCH);
                    move_immediate(code, scratch, sign_extended(value));
                    emit(
                        code,
                        three_registers(ADD_REGISTER_SETTING_FLAGS, target, target, scratch),
                    );
                }
            }
            Operation::Subtract { target, source } => {
                let (target, source) =
                    (general(target, "a subtract"), general(source, "a subtract"));
                emit(
                    code,
                    three_registers(SUBTRACT_REGISTER_SETTING_FLAGS, target, target, source),
                );
            }
            Operation::And { target, source } => {
                let (target, source) = (general(target, "an and"), general(source, "an and"));
                emit(code, three_registers(AND_REGISTER, target, target, source));
            }
            Operation::AndImmediate { target, value } => {
                logical(code, AND_IMMEDIATE, AND_REGISTER, target, value);
            }
            Operation::OrImmediate { target, value } => {
                logical(code, OR_IMMEDIATE, OR_REGISTER, target, value);
            }
            Operation::ShiftLeft { target, count } => {
                let count = shift_count(count);
                bitfield_move(
                    code,
                    UNSIGNED_BITFIELD_MOVE,
                    target,
                    (REGISTER_BITS - count) % REGISTER_BITS,
                    HIGHEST_BIT - count,
                );
            }
            Operation::ShiftRightLogical { target, count } => {
                let count = shift_count(count);
                bitfield_move(code, UNSIGNED_BITFIELD_MOVE, target, count, HIGHEST_BIT);
            }
            Operation::ShiftRightArithmetic { target, count } => {
                let count = shift_count(count);
                bitfield_move(code, SIGNED_BITFIELD_MOVE, target, count, HIGHEST_BIT);
            }
            Operation::Compare { left, right } => {
                let mut right = general_or_stack(right);
                let left = general_or_stack(left);
                if right == own(SP) {
                    // No compare reads a stack pointer on the right.
                    let scratch = own(SCRATCH);
                    move_through_add(code, scratch, right);
                    right = scratch;
                }
                compare_registers(code, left, right);
            }
            Operation::CompareImmediate { left, right } => {
                let left = general_or_stack(left);
                // `cmn` of the magnitude sets every flag as `cmp` of a
                // negative value does.
                if let Some(opcode) = signed_arithmetic(
                    right,
                    SUBTRACT_IMMEDIATE_SETTING_FLAGS,
                    ADD_IMMEDIATE_SETTING_FLAGS,
                ) {
                    emit(code, three_registers(opcode, ZERO_REGISTER, left, 0));
                } else {
                    let scratch = own(SCRATCH);
                    move_immediate(code, scratch, sign_extended(right));
                    compare_registers(code, left, scratch);
                }
            }
            Operation::CompareMemory { left, right } => {
                let left = general_or_stack(left);
                // The second scratch register, since the first may carry
                // the displacement.
                let second = own(SECOND_SCRATCH);
                access(code, Width::Word64, true, second, right);
                compare_registers(code, left, second);
            }
            Operation::TestImmediate { width, left, right } => {
                test_immediate(code, width, left, right);
            }
            Operation::AddStack { amount } => {
                let stack = own(SP);
                // `add` and `sub`, never `adds`: the operation keeps the
                // flags.
                if let Some(opcode) = signed_arithmetic(amount, ADD_IMMEDIATE, SUBTRACT_IMMEDIATE) {
                    emit(code, three_registers(opcode, stack, stack, 0));
                } else {
                    let scratch = own(SCRATCH);
                    move_immediate(code, scratch, sign_extended(amount));
                    emit(
                        code,
                        three_registers(ADD_EXTENDED_REGISTER, stack, stack, scratch),
                    );
                }
            }
            Operation::MultiplyChecked { target, source } => {
                let (target, source) =
                    (general(target, "a multiply"), general(source, "a multiply"));
                let high = own(SCRATCH);
                // The high word first, while `target` still holds the
                // factor.
                emit(
                    code,
                    three_registers(SIGNED_MULTIPLY_HIGH, high, target, source),
                );
                emit(code, three_registers(MULTIPLY, target, target, source));
                // The product fits exactly when the high word is the low
                // word's sign.
                emit(
                    code,
                    three_registers(
                        SUBTRACT_REGISTER_SETTING_FLAGS | SHIFTED_ARITHMETIC_RIGHT,
                        ZERO_REGISTER,
                        high,
                        target,
                    ) | (HIGHEST_BIT << SHIFT_AMOUNT_SHIFT),
                );
                // On equal, zero minus zero clears V; otherwise V alone is
                // set, so `Overflow` and `NoOverflow` read the fit.
                emit(
                    code,
                    three_registers(
                        CONDITIONAL_COMPARE_REGISTER,
                        FLAGS_OVERFLOW_ALONE,
                        ZERO_REGISTER,
                        ZERO_REGISTER,
                    ) | (CONDITION_EQUAL << CONDITION_SHIFT),
                );
            }
            Operation::DivideSigned {
                dividend,
                remainder,
                divisor,
            } => {
                // The remainder is written while the dividend and the
                // divisor are still read, so no two may share a register.
                assert!(
                    dividend != remainder && dividend != divisor && remainder != divisor,
                    "a division takes three distinct registers, not x{}, x{} and x{}",
                    dividend.0,
                    remainder.0,
                    divisor.0
                );
                let (dividend, remainder, divisor) = (
                    general(dividend, "a division"),
                    general(remainder, "a division"),
                    general(divisor, "a division"),
                );
                let quotient = own(SCRATCH);
                emit(
                    code,
                    three_registers(SIGNED_DIVIDE, quotient, dividend, divisor),
                );
                emit(
                    code,
                    three_registers(MULTIPLY_SUBTRACT, remainder, quotient, divisor)
                        | (dividend << ADDEND_SHIFT),
                );
                emit(code, three_registers(MOVE_REGISTER, dividend, 0, quotient));
            }
            Operation::CallRegister { target } => {
                let target = general(target, "a call");
                emit(
                    code,
                    three_registers(BRANCH_WITH_LINK_REGISTER, 0, target, 0),
                );
            }
            Operation::JumpRegister { target } => {
                let target = general(target, "a jump");
                emit(code, three_registers(BRANCH_REGISTER, 0, target, 0));
            }
            Operation::EnterCalled => push(code, own(X30)),
            Operation::JumpIntoCalled { target } => {
                let target = general(target, "a jump");
                let skipping = own(SCRATCH);
                // `add`, never `adds`: the jump keeps the flags. The save
                // [`Operation::EnterCalled`] encodes is one instruction.
                emit(
                    code,
                    three_registers(ADD_IMMEDIATE, skipping, target, 0)
                        | (INSTRUCTION_BYTES << ARITHMETIC_IMMEDIATE_SHIFT),
                );
                emit(code, three_registers(BRANCH_REGISTER, 0, skipping, 0));
            }
            Operation::Push { source } => push(code, general(source, "a push")),
            Operation::Pop { target } => pop(code, general(target, "a pop")),
            Operation::LoadCodeAddress { target } => {
                let target = general(target, "a code address");
                let Some(instruction) = address(target, i64::from(INSTRUCTION_BYTES)) else {
                    panic!("an adr reaches the next instruction");
                };
                emit(code, instruction);
            }
            Operation::Return => {
                pop(code, own(X30));
                emit(code, RETURN_TO_LINK_REGISTER);
            }
            Operation::Trap => emit(code, UNDEFINED),
        }
    }

    fn encode_branch(
        &self,
        kind: BranchKind,
        _form: BranchForm,
        distance: i64,
        code: &mut Vec<u8>,
    ) -> bool {
        // Both forms are one instruction, so the long form is the short
        // one: a conditional branch beyond `b.cond`'s mebibyte answers
        // `false` rather than becoming an inverted `b.cond` over a `b`,
        // which would make every forward check a taken branch.
        let instruction = match kind {
            BranchKind::Unconditional => {
                instruction_field(distance, BRANCH_DISTANCE_BITS).map(|field| BRANCH | field)
            }
            BranchKind::Conditional(condition) => {
                instruction_field(distance, CONDITIONAL_DISTANCE_BITS).map(|field| {
                    BRANCH_CONDITIONAL
                        | (field << CONDITIONAL_DISTANCE_SHIFT)
                        | condition_code(condition)
                })
            }
        };
        match instruction {
            Some(instruction) => {
                emit(code, instruction);
                true
            }
            None => false,
        }
    }

    fn encode_label_reference(
        &self,
        reference: LabelReference,
        distance: i64,
        code: &mut Vec<u8>,
    ) -> bool {
        let instruction = match reference {
            LabelReference::Load { target } => {
                let target = general(target, "a literal load");
                instruction_field(distance, CONDITIONAL_DISTANCE_BITS).map(|field| {
                    LOAD_LITERAL
                        | (field << CONDITIONAL_DISTANCE_SHIFT)
                        | (target << DESTINATION_SHIFT)
                })
            }
            LabelReference::Address { target } => address(general(target, "an address"), distance),
        };
        match instruction {
            Some(instruction) => {
                emit(code, instruction);
                true
            }
            None => false,
        }
    }
}
