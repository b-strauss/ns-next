//! The abstract machine operations ns-next's JIT emits, and one encoder per
//! architecture.
//!
//! The crate knows nothing of the VM: an operation is a move, a load or a
//! store of one of the widths the heap uses, an arithmetic or logical
//! operation, a compare, a jump to a label, a call, a push or a pop. Every
//! tier writes its code in these operations, so a second architecture is a
//! second [`Encoder`] and nothing above it changes
//! (`docs/design/execution.md`, "The foundation and the one extension
//! point").
//!
//! An operation's encoding may be several machine instructions -- an
//! AArch64 64-bit immediate, or the overflow check of its multiply, takes
//! more than one -- so an encoder answers bytes, never one instruction.
//! AArch64 materializes a constant in place with a `movz`/`movk` sequence of
//! at most four instructions and keeps no literal pool: the immediates the
//! JIT embeds are small (tagged integers, 22-bit dispatch ids, offsets), so
//! a pool would rarely save an instruction, and without one an operation's
//! bytes are self-contained and [`Encoder`] admits AArch64 as it is. A
//! branch's distance is known only once every operation before its target
//! is, so branches are encoded in [`Assembler::finish`], in one pass: a
//! backward branch, whose target is already placed, in its shortest form
//! that reaches, a forward one in its longest, patched once its target is
//! placed. A forward branch never shrinks, so no offset placed before it
//! ever moves and one pass suffices.
//!
//! Every constant of an encoder -- an opcode, a prefix, an opcode extension,
//! a condition code, a field's bits, an instruction's size -- is a named
//! `const` documented with its official source: the Intel SDM volume and
//! section title for x64, the developer.arm.com page of the instruction for
//! AArch64. A reader checks a byte against the manual instead of trusting
//! it. `tests/encoding_sources.rs` (`every_encoding_constant_is_named_and_sourced`)
//! holds every encoder file under `src/`, subdirectories included, to it:
//! each `const` cites its source, and no integer literal other than 0 and 1
//! stands outside a `const`.

#![forbid(unsafe_code)]

use std::fmt;

pub mod aarch64;
pub mod x64;

pub use aarch64::Aarch64;
pub use x64::X64;

/// A machine register, numbered as the target architecture numbers it in
/// its encodings ([`x64::RAX`] … [`x64::R15`], [`aarch64::X0`] …
/// [`aarch64::X30`] and [`aarch64::SP`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Register(pub u8);

/// The width of a load or a store. A load of less than a word fills the
/// register's upper bits with zeros.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Width {
    /// Eight bits.
    Byte,
    /// Sixteen bits.
    Half,
    /// Thirty-two bits.
    Word32,
    /// Sixty-four bits.
    Word64,
}

/// A memory operand: the address `base + displacement`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Memory {
    /// The register holding the base address.
    pub base: Register,
    /// The signed byte offset from the base.
    pub displacement: i32,
}

/// The condition of a conditional jump, read from the condition flags the
/// last operation that set them left.
///
/// Every operation sets, keeps or clobbers the flags, and every
/// [`Encoder`] honours the same contract, so a condition means the same on
/// every architecture:
///
/// - **Sets, every condition defined**: `compare`, `compare_immediate` and
///   `compare_memory`. The signed conditions (`Less` … `GreaterOrEqual`)
///   and the unsigned ones (`Below` … `AboveOrEqual`) compare `left` with
///   `right`; `Overflow` holds when `left - right` overflows.
/// - **Sets, only `Equal` and `NotEqual` defined**: `test_immediate` and
///   `test_immediate_with_width`.
/// - **Sets, only `Overflow` and `NoOverflow` defined**: `add`,
///   `add_immediate`, `subtract` and `multiply_checked`. No other
///   condition may be read after them: the carry of an addition means
///   opposite things on x64 and AArch64. AArch64 has no multiply that
///   sets the flags, and honours `Overflow` after `multiply_checked`
///   with four instructions: `smulh` for the product's high word into a
///   register the encoder keeps for itself, then `mul` for its low word
///   into `target`; a `cmp` of the
///   high word with the low word's sign (`asr #63`), equal exactly when
///   the product fits; then `ccmp xzr, xzr, #0b0001, eq`, which on equal
///   compares zero with zero, clearing V, and otherwise sets the flags
///   to V alone. So the contract holds as x64's `imul` sets it, and the
///   operation needs no other condition.
/// - **Clobbers**, the flags undefined afterwards: `and`, `and_immediate`,
///   `or_immediate`, `shift_left`, `shift_right_arithmetic`,
///   `shift_right_logical` -- x64's
///   instructions set the flags, AArch64's plain `and`, `orr`, `lsl` and
///   `asr` keep them, so neither may be relied on -- `divide_signed`,
///   whose flags x64 leaves undefined, and `call_register` and
///   `call_unentered`, whose callee may do anything.
/// - **Keeps**: every other operation -- `move_register`, `move_if`,
///   `move_immediate`, `load`, `store`, `store_immediate32`, `push`, `pop`,
///   `add_stack`, `load_code_address`, `load_labelled`, `load_label_address`, `jump`, `jump_if`,
///   `jump_register`, `enter_called`, `jump_into_called`, `return_` and `trap` -- so a compare
///   may be separated from its branch by moves, loads and stores.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Condition {
    /// Equal; zero after `test_immediate`.
    Equal,
    /// Not equal; non-zero after `test_immediate`.
    NotEqual,
    /// Signed less than.
    Less,
    /// Signed less than or equal.
    LessOrEqual,
    /// Signed greater than.
    Greater,
    /// Signed greater than or equal.
    GreaterOrEqual,
    /// Unsigned less than.
    Below,
    /// Unsigned less than or equal.
    BelowOrEqual,
    /// Unsigned greater than.
    Above,
    /// Unsigned greater than or equal.
    AboveOrEqual,
    /// Signed overflow of the last arithmetic operation.
    Overflow,
    /// No signed overflow of the last arithmetic operation.
    NoOverflow,
}

/// A position in the code, named before it is placed. Only the assembler
/// that made it may bind it or branch to it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Label(u32);

impl Label {
    /// The label's index into [`Assembled::label_offsets`].
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// An operation other than a branch, as an [`Encoder`] receives it. The
/// fields are named after the operand they are: `target` is written,
/// `source`, `left` and `right` are only read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Operation {
    /// `target := source`.
    MoveRegister { target: Register, source: Register },
    /// `target := source` when `condition` holds; `target` unchanged
    /// otherwise.
    MoveIf {
        condition: Condition,
        target: Register,
        source: Register,
    },
    /// `target := value`.
    MoveImmediate { target: Register, value: u64 },
    /// `target := [source]`, zero-extended from `width`.
    Load {
        width: Width,
        target: Register,
        source: Memory,
    },
    /// `[target] := source`'s low `width` bits.
    Store {
        width: Width,
        target: Memory,
        source: Register,
    },
    /// The 64-bit word at `target` := `value` sign-extended.
    StoreImmediate32 { target: Memory, value: i32 },
    /// `target := target + source`, setting `Overflow`.
    Add { target: Register, source: Register },
    /// `target := target + value` sign-extended, setting `Overflow`.
    AddImmediate { target: Register, value: i32 },
    /// `target := target - source`, setting `Overflow`.
    Subtract { target: Register, source: Register },
    /// `target := target * source`, signed; `Overflow` when the product
    /// does not fit 64 bits.
    MultiplyChecked { target: Register, source: Register },
    /// `dividend := dividend / divisor`, signed and truncated toward zero,
    /// and `remainder :=` what is left, with the dividend's sign. The
    /// divisor is neither zero nor -1 under the most negative dividend;
    /// an encoder may require particular registers for the three
    /// operands (`Assembler::divide_signed`).
    DivideSigned {
        dividend: Register,
        remainder: Register,
        divisor: Register,
    },
    /// `target := target & source`.
    And { target: Register, source: Register },
    /// `target := target & value` sign-extended.
    AndImmediate { target: Register, value: i32 },
    /// `target := target | value` sign-extended.
    OrImmediate { target: Register, value: i32 },
    /// `target := target << count`, `count` below 64.
    ShiftLeft { target: Register, count: u8 },
    /// `target := target >> count`, the sign copied in, `count` below 64.
    ShiftRightArithmetic { target: Register, count: u8 },
    /// `target := target >> count`, zeros shifted in, `count` below 64.
    ShiftRightLogical { target: Register, count: u8 },
    /// Sets the flags as `left - right` does.
    Compare { left: Register, right: Register },
    /// Sets the flags as `left - right` sign-extended does.
    CompareImmediate { left: Register, right: i32 },
    /// Sets the flags as `left - [right]`, a 64-bit word, does.
    CompareMemory { left: Register, right: Memory },
    /// Sets `Equal` when `left & right` is zero: of 64 bits, `right`
    /// sign-extended, for `Width::Word64`; of the low 32 bits for
    /// `Width::Word32`, so that bit 31 is tested alone. No other width.
    TestImmediate {
        width: Width,
        left: Register,
        right: i32,
    },
    /// Calls the address in `target`.
    CallRegister { target: Register },
    /// Calls the address in `target`, which does not begin with
    /// [`Operation::EnterCalled`] -- a position inside a body -- saving the
    /// return address where that operation would, so that the callee's
    /// [`Operation::Return`] returns to the operation after this one.
    CallUnentered { target: Register },
    /// Jumps to the address in `target`.
    JumpRegister { target: Register },
    /// The first operation of every address a call reaches: after it the
    /// return address the call left lies at `[stack pointer + 0]`, where
    /// [`Operation::Return`] expects it. An architecture whose call already
    /// leaves it there encodes nothing.
    EnterCalled,
    /// Jumps to the address in `target`, which begins with
    /// [`Operation::EnterCalled`], past that operation: the return address
    /// is already saved where the callee expects it. A stub's tail jump
    /// into a callee.
    JumpIntoCalled { target: Register },
    /// Pushes `source` on the machine stack.
    Push { source: Register },
    /// Pops the machine stack into `target`.
    Pop { target: Register },
    /// Adds `amount` bytes to the stack pointer, keeping the flags.
    AddStack { amount: i32 },
    /// `target :=` the address of the operation that follows this one:
    /// an address inside the code, which needs no relocation wherever the
    /// code is placed.
    LoadCodeAddress { target: Register },
    /// Returns to the caller.
    Return,
    /// Stops the program with an illegal-instruction fault.
    Trap,
}

/// An operation whose operand is an address in the code, named by a
/// label: `displacement` bytes past it, reached relative to the
/// operation's own position, so that the code needs no relocation
/// wherever it is placed. Its distance is known only once the code is
/// laid out, so it is encoded in [`Assembler::finish`], as a branch is,
/// in one form whose size does not depend on the distance.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LabelReference {
    /// `target :=` the 64-bit word at the address.
    Load { target: Register },
    /// `target :=` the address itself.
    Address { target: Register },
}

/// What a branch does at its target.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BranchKind {
    /// Always jumps.
    Unconditional,
    /// Jumps when the condition holds, falls through otherwise.
    Conditional(Condition),
}

/// The two forms of a branch: one that reaches near targets in fewer
/// bytes, one that reaches every target in the code.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BranchForm {
    /// The shortest form, which reaches only near targets.
    Short,
    /// The longest form, whose size does not depend on its distance.
    Long,
}

/// The encoding of the operations for one architecture.
///
/// An encoder may emit several instructions for one operation, and may
/// keep registers of its own out of the operands it is given (AArch64's
/// x16 and x17, its scratch registers, x18 and x30), but it honours
/// [`Condition`]'s contract on the flags exactly: an operation that keeps
/// them emits nothing that changes them, one that sets them defines every
/// condition the contract names for it.
pub trait Encoder {
    /// Appends the machine code of `operation` to `code`.
    ///
    /// # Panics
    ///
    /// On an operand the architecture has no encoding for -- a register
    /// number it does not have, a shift count of 64 or more: a defect of
    /// the code generator, never of the program it compiles.
    fn encode(&self, operation: Operation, code: &mut Vec<u8>);

    /// Appends `kind`'s branch in `form` to `code`, its target `distance`
    /// bytes from the branch's first byte, and answers whether that form
    /// reaches it; when it does not, appends nothing. The long form's size
    /// must not depend on `distance`, since a forward branch is patched in
    /// place.
    fn encode_branch(
        &self,
        kind: BranchKind,
        form: BranchForm,
        distance: i64,
        code: &mut Vec<u8>,
    ) -> bool;

    /// Appends `reference`, its address `distance` bytes from the
    /// operation's first byte, and answers whether its form reaches it;
    /// when it does not, appends nothing. Its size must not depend on
    /// `distance`, since a forward reference is patched in place. Keeps
    /// the flags.
    fn encode_label_reference(
        &self,
        reference: LabelReference,
        distance: i64,
        code: &mut Vec<u8>,
    ) -> bool;

    /// Appends `length` bytes that stop the program wherever execution
    /// enters them, so that code no branch is to reach -- the room a
    /// moved or freed body left -- traps at once rather than running what
    /// lay there before.
    fn fill_with_stops(&self, length: usize, code: &mut Vec<u8>);

    /// The bytes one return address, or one pushed register, takes on the
    /// machine stack: the stack pointer moves by multiples of it.
    fn stack_slot_bytes(&self) -> i32;

    /// The alignment, in bytes, every instruction of the code must have in
    /// memory: 1 where instructions have any address.
    fn instruction_alignment(&self) -> u32;
}

/// The code an [`Assembler`] produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Assembled {
    /// The machine code, position-independent: every branch is relative.
    pub bytes: Vec<u8>,
    /// For each label, by [`Label::index`], the offset in `bytes` it was
    /// bound at, `None` for one never bound.
    pub label_offsets: Vec<Option<u32>>,
    /// The alignment the code's first byte must have in memory for every
    /// [`Assembler::align_data`] to align there too, and the encoder's
    /// [`Encoder::instruction_alignment`] to hold: the largest asked
    /// for, 1 when none was.
    pub alignment: u32,
}

/// Why [`Assembler::finish`] produced no code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssemblyError {
    /// A branch names a label that was never bound.
    UnboundLabel(Label),
    /// A branch's target lies beyond the reach of its longest form, or a
    /// label reference's beyond the reach of its one.
    BranchOutOfReach(Label),
    /// The code is larger than the offsets of [`Assembled`] count.
    CodeTooLarge,
}

impl fmt::Display for AssemblyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnboundLabel(label) => {
                write!(
                    formatter,
                    "a branch names label {}, which was never bound",
                    label.0
                )
            }
            Self::BranchOutOfReach(label) => {
                write!(formatter, "label {} lies beyond a branch's reach", label.0)
            }
            Self::CodeTooLarge => write!(formatter, "the code exceeds 4 GiB"),
        }
    }
}

impl std::error::Error for AssemblyError {}

/// One element of the code in the order it was emitted: encoded operations,
/// a branch still to be sized, a label's position, a label reference still
/// to be encoded, or data placed after the code.
#[derive(Clone, Debug)]
enum Item {
    /// The range of `Assembler::encoded` that holds consecutive operations.
    Code {
        start: usize,
        end: usize,
    },
    Branch {
        kind: BranchKind,
        label: Label,
    },
    Bind(Label),
    Reference {
        reference: LabelReference,
        label: Label,
        displacement: i32,
    },
    /// Zero bytes up to the next multiple of this many from the start.
    Align(u32),
    /// This many zero bytes.
    Data(u32),
}

/// Collects operations, labels and branches, and lays them out in
/// [`Assembler::finish`].
#[derive(Debug)]
pub struct Assembler<E: Encoder> {
    encoder: E,
    /// Every operation's bytes, encoded as it was emitted.
    encoded: Vec<u8>,
    items: Vec<Item>,
    /// Whether each label is bound yet, by number.
    bound: Vec<bool>,
    /// The largest alignment `align_data` asked for.
    alignment: u32,
}

impl<E: Encoder + Default> Default for Assembler<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: Encoder + Default> Assembler<E> {
    /// An assembler with no code, over the encoder's default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::with_encoder(E::default())
    }
}

impl<E: Encoder> Assembler<E> {
    /// An assembler with no code, over `encoder` as configured for its
    /// target.
    #[must_use]
    pub fn with_encoder(encoder: E) -> Self {
        Self {
            encoder,
            encoded: Vec::new(),
            items: Vec::new(),
            bound: Vec::new(),
            alignment: 1,
        }
    }

    /// The encoder, for what depends on the target: its stack slot, its
    /// instruction alignment.
    #[must_use]
    pub fn encoder(&self) -> &E {
        &self.encoder
    }

    /// A new label, not yet bound.
    ///
    /// # Panics
    ///
    /// When the assembler already made `u32::MAX` labels.
    pub fn label(&mut self) -> Label {
        let Ok(number) = u32::try_from(self.bound.len()) else {
            panic!("an assembler makes at most u32::MAX labels");
        };
        self.bound.push(false);
        Label(number)
    }

    /// Places `label` at the next operation.
    ///
    /// # Panics
    ///
    /// When `label` is already bound, or another assembler made it.
    pub fn bind(&mut self, label: Label) {
        let Some(bound) = self.bound.get_mut(label.index()) else {
            panic!("label {} belongs to another assembler", label.0);
        };
        assert!(!*bound, "label {} bound twice", label.0);
        *bound = true;
        self.items.push(Item::Bind(label));
    }

    fn emit(&mut self, operation: Operation) {
        let start = self.encoded.len();
        self.encoder.encode(operation, &mut self.encoded);
        let end = self.encoded.len();
        if let Some(Item::Code { end: last_end, .. }) = self.items.last_mut() {
            if *last_end == start {
                *last_end = end;
                return;
            }
        }
        self.items.push(Item::Code { start, end });
    }

    fn branch(&mut self, kind: BranchKind, label: Label) {
        assert!(
            label.index() < self.bound.len(),
            "label {} belongs to another assembler",
            label.0
        );
        self.items.push(Item::Branch { kind, label });
    }

    /// `target := source`. Keeps the flags.
    pub fn move_register(&mut self, target: Register, source: Register) {
        self.emit(Operation::MoveRegister { target, source });
    }

    /// `target := source` when `condition` holds, as the flags the last
    /// operation that set them define it; `target` keeps its value
    /// otherwise. Keeps the flags, so one compare may decide several
    /// moves.
    pub fn move_if(&mut self, condition: Condition, target: Register, source: Register) {
        self.emit(Operation::MoveIf {
            condition,
            target,
            source,
        });
    }

    /// `target := value`. Never touches the flags, which is why x64 never
    /// clears a register with `xor` here: a constant may be loaded between a
    /// compare and its branch.
    pub fn move_immediate(&mut self, target: Register, value: u64) {
        self.emit(Operation::MoveImmediate { target, value });
    }

    /// `target := [source]`, zero-extended from `width`. Keeps the flags.
    pub fn load(&mut self, width: Width, target: Register, source: Memory) {
        self.emit(Operation::Load {
            width,
            target,
            source,
        });
    }

    /// `[target] := source`'s low `width` bits. Keeps the flags.
    pub fn store(&mut self, width: Width, target: Memory, source: Register) {
        self.emit(Operation::Store {
            width,
            target,
            source,
        });
    }

    /// The 64-bit word at `target` := `value` sign-extended. Keeps the
    /// flags.
    pub fn store_immediate32(&mut self, target: Memory, value: i32) {
        self.emit(Operation::StoreImmediate32 { target, value });
    }

    /// `target := target + source`. Sets the flags; only `Overflow` and
    /// `NoOverflow` are defined after it.
    pub fn add(&mut self, target: Register, source: Register) {
        self.emit(Operation::Add { target, source });
    }

    /// `target := target + value` sign-extended. Sets the flags; only
    /// `Overflow` and `NoOverflow` are defined after it.
    pub fn add_immediate(&mut self, target: Register, value: i32) {
        self.emit(Operation::AddImmediate { target, value });
    }

    /// `target := target - source`. Sets the flags; only `Overflow` and
    /// `NoOverflow` are defined after it.
    pub fn subtract(&mut self, target: Register, source: Register) {
        self.emit(Operation::Subtract { target, source });
    }

    /// `target := target * source`, signed. Sets the flags: `Overflow`
    /// when the product does not fit 64 bits; only `Overflow` and
    /// `NoOverflow` are defined after it.
    pub fn multiply_checked(&mut self, target: Register, source: Register) {
        self.emit(Operation::MultiplyChecked { target, source });
    }

    /// `dividend := dividend / divisor`, signed and truncated toward zero,
    /// and `remainder := dividend - quotient * divisor`, which takes the
    /// dividend's sign. Clobbers the flags.
    ///
    /// The caller excludes a zero divisor, and -1 under the most negative
    /// dividend, whose quotient does not fit: x64 faults on both. An
    /// encoder may fix the operands' registers -- x64's `idiv` divides
    /// [`x64::DIVIDEND`] and leaves the remainder in [`x64::REMAINDER`]
    /// -- and panics on others, as on a register it has no encoding for.
    pub fn divide_signed(&mut self, dividend: Register, remainder: Register, divisor: Register) {
        self.emit(Operation::DivideSigned {
            dividend,
            remainder,
            divisor,
        });
    }

    /// `target := target & source`. Clobbers the flags.
    pub fn and(&mut self, target: Register, source: Register) {
        self.emit(Operation::And { target, source });
    }

    /// `target := target & value` sign-extended. Clobbers the flags.
    pub fn and_immediate(&mut self, target: Register, value: i32) {
        self.emit(Operation::AndImmediate { target, value });
    }

    /// `target := target | value` sign-extended. Clobbers the flags.
    pub fn or_immediate(&mut self, target: Register, value: i32) {
        self.emit(Operation::OrImmediate { target, value });
    }

    /// `target := target << count`; `count` must be below 64. Clobbers
    /// the flags.
    pub fn shift_left(&mut self, target: Register, count: u8) {
        self.emit(Operation::ShiftLeft { target, count });
    }

    /// `target := target >> count`, the sign copied in; `count` must be
    /// below 64. Clobbers the flags.
    pub fn shift_right_arithmetic(&mut self, target: Register, count: u8) {
        self.emit(Operation::ShiftRightArithmetic { target, count });
    }

    /// `target := target >> count`, zeros shifted in; `count` must be
    /// below 64. Clobbers the flags.
    pub fn shift_right_logical(&mut self, target: Register, count: u8) {
        self.emit(Operation::ShiftRightLogical { target, count });
    }

    /// Sets the flags to compare `left` with `right`; every condition is
    /// defined after it.
    pub fn compare(&mut self, left: Register, right: Register) {
        self.emit(Operation::Compare { left, right });
    }

    /// Sets the flags to compare `left` with `right` sign-extended; every
    /// condition is defined after it.
    pub fn compare_immediate(&mut self, left: Register, right: i32) {
        self.emit(Operation::CompareImmediate { left, right });
    }

    /// Sets the flags to compare `left` with the 64-bit word at `right`;
    /// every condition is defined after it.
    pub fn compare_memory(&mut self, left: Register, right: Memory) {
        self.emit(Operation::CompareMemory { left, right });
    }

    /// Sets the flags: `Equal` when `left & right` sign-extended is zero,
    /// `NotEqual` otherwise; no other condition is defined after it.
    pub fn test_immediate(&mut self, left: Register, right: i32) {
        self.test_immediate_with_width(Width::Word64, left, right);
    }

    /// Sets the flags as `test_immediate` does, over `width`'s low bits of
    /// `left`: `Width::Word64`, `right` sign-extended, or `Width::Word32`,
    /// `right` as its 32 bits, so that a mask of bit 31 tests that bit
    /// alone rather than the upper half too.
    ///
    /// # Panics
    ///
    /// For a byte or half-word width, which no operation tests.
    pub fn test_immediate_with_width(&mut self, width: Width, left: Register, right: i32) {
        assert!(
            matches!(width, Width::Word32 | Width::Word64),
            "a test is of 32 or 64 bits"
        );
        self.emit(Operation::TestImmediate { width, left, right });
    }

    /// Jumps to `label`. Keeps the flags.
    pub fn jump(&mut self, label: Label) {
        self.branch(BranchKind::Unconditional, label);
    }

    /// Jumps to `label` when `condition` holds. Keeps the flags.
    pub fn jump_if(&mut self, condition: Condition, label: Label) {
        self.branch(BranchKind::Conditional(condition), label);
    }

    /// Calls the address in `target`. Clobbers the flags: the callee may
    /// do anything with them.
    pub fn call_register(&mut self, target: Register) {
        self.emit(Operation::CallRegister { target });
    }

    /// Calls the address in `target`, which does not begin with
    /// [`Assembler::enter_called`] -- a position inside a body, reached by
    /// fall-through and branches too -- saving the return address where
    /// `enter_called` would, so that the callee's [`Assembler::return_`]
    /// returns to the operation after this one. Clobbers the flags: the
    /// callee may do anything with them.
    pub fn call_unentered(&mut self, target: Register) {
        self.emit(Operation::CallUnentered { target });
    }

    /// Jumps to the address in `target`, pushing nothing: a jump that
    /// never comes back, which a `call_register` would leave unbalanced on
    /// the processor's return-address predictor. Keeps the flags.
    pub fn jump_register(&mut self, target: Register) {
        self.emit(Operation::JumpRegister { target });
    }

    /// The first operation of every address a call reaches: afterwards the
    /// return address the call left lies at `[stack pointer + 0]`, where
    /// [`Assembler::return_`] expects it. Keeps the flags.
    pub fn enter_called(&mut self) {
        self.emit(Operation::EnterCalled);
    }

    /// Jumps to the address in `target`, which begins with
    /// [`Assembler::enter_called`], past that operation: the return address
    /// is already saved where it expects it. A stub's tail jump into a
    /// callee. Keeps the flags.
    pub fn jump_into_called(&mut self, target: Register) {
        self.emit(Operation::JumpIntoCalled { target });
    }

    /// Pushes `source` on the machine stack. Keeps the flags.
    pub fn push(&mut self, source: Register) {
        self.emit(Operation::Push { source });
    }

    /// Pops the machine stack into `target`. Keeps the flags.
    pub fn pop(&mut self, target: Register) {
        self.emit(Operation::Pop { target });
    }

    /// Adds `amount` bytes to the stack pointer. Keeps the flags, so a
    /// frame may be released between a compare and its branch.
    pub fn add_stack(&mut self, amount: i32) {
        self.emit(Operation::AddStack { amount });
    }

    /// `target :=` the address of the operation emitted next: an address
    /// inside this code, wherever it is placed, so that what is handed it
    /// can tell which code it came from. Keeps the flags.
    pub fn load_code_address(&mut self, target: Register) {
        self.emit(Operation::LoadCodeAddress { target });
    }

    /// `target :=` the 64-bit word `displacement` bytes past `label`, an
    /// address in this code reached relative to the operation, so that it
    /// needs no relocation wherever the code is placed: data the code
    /// keeps after its last instruction (`align_data`, `zeroed_data`).
    /// Keeps the flags.
    ///
    /// # Panics
    ///
    /// When another assembler made `label`.
    pub fn load_labelled(&mut self, target: Register, label: Label, displacement: i32) {
        self.reference(LabelReference::Load { target }, label, displacement);
    }

    /// `target :=` the address `displacement` bytes past `label`, as
    /// `load_labelled` reaches it. Keeps the flags.
    ///
    /// # Panics
    ///
    /// When another assembler made `label`.
    pub fn load_label_address(&mut self, target: Register, label: Label, displacement: i32) {
        self.reference(LabelReference::Address { target }, label, displacement);
    }

    fn reference(&mut self, reference: LabelReference, label: Label, displacement: i32) {
        assert!(
            label.index() < self.bound.len(),
            "label {} belongs to another assembler",
            label.0
        );
        self.items.push(Item::Reference {
            reference,
            label,
            displacement,
        });
    }

    /// Pads the code with zero bytes up to the next multiple of
    /// `alignment` bytes from its start, for data placed after its last
    /// instruction, which control never reaches. The padding aligns in
    /// memory once the code's first byte does ([`Assembled::alignment`]).
    ///
    /// # Panics
    ///
    /// When `alignment` is not a power of two.
    pub fn align_data(&mut self, alignment: u32) {
        assert!(
            alignment.is_power_of_two(),
            "an alignment is a power of two, not {alignment}"
        );
        self.alignment = self.alignment.max(alignment);
        self.items.push(Item::Align(alignment));
    }

    /// Appends `byte_count` zero bytes: data the code reaches by a label
    /// (`load_labelled`), after its last instruction, which control never
    /// reaches.
    pub fn zeroed_data(&mut self, byte_count: u32) {
        self.items.push(Item::Data(byte_count));
    }

    /// Returns to the caller. Keeps the flags.
    pub fn return_(&mut self) {
        self.emit(Operation::Return);
    }

    /// Stops the program with an illegal-instruction fault. Keeps the
    /// flags.
    pub fn trap(&mut self) {
        self.emit(Operation::Trap);
    }

    /// Lays the code out, sizing every branch.
    ///
    /// # Errors
    ///
    /// [`AssemblyError::UnboundLabel`] when a branch names a label never
    /// bound, [`AssemblyError::BranchOutOfReach`] when a target lies beyond
    /// a branch's longest form, [`AssemblyError::CodeTooLarge`] when an
    /// offset does not fit 32 bits.
    pub fn finish(self) -> Result<Assembled, AssemblyError> {
        let mut bytes = Vec::with_capacity(self.encoded.len());
        let mut label_offsets: Vec<Option<u32>> = vec![None; self.bound.len()];
        let mut forward = Vec::new();
        let mut forward_references = Vec::new();
        for item in self.items {
            match item {
                Item::Code { start, end } => bytes.extend_from_slice(&self.encoded[start..end]),
                Item::Align(alignment) => {
                    let alignment = alignment as usize;
                    bytes.resize(bytes.len().next_multiple_of(alignment), 0);
                }
                Item::Data(byte_count) => bytes.resize(bytes.len() + byte_count as usize, 0),
                Item::Reference {
                    reference,
                    label,
                    displacement,
                } => {
                    let start = bytes.len();
                    let target = label_offsets[label.index()];
                    // A forward reference is encoded at distance 0 and
                    // patched once its label is placed.
                    let distance = match target {
                        Some(target) => reference_distance(start, target, displacement)?,
                        None => 0,
                    };
                    if !self
                        .encoder
                        .encode_label_reference(reference, distance, &mut bytes)
                    {
                        return Err(AssemblyError::BranchOutOfReach(label));
                    }
                    if target.is_none() {
                        forward_references.push((
                            start,
                            bytes.len(),
                            reference,
                            label,
                            displacement,
                        ));
                    }
                }
                Item::Bind(label) => {
                    let offset =
                        u32::try_from(bytes.len()).map_err(|_| AssemblyError::CodeTooLarge)?;
                    label_offsets[label.index()] = Some(offset);
                }
                Item::Branch { kind, label } => {
                    let start = bytes.len();
                    if let Some(target) = label_offsets[label.index()] {
                        let distance = distance(start, target)?;
                        let reaches = self.encoder.encode_branch(
                            kind,
                            BranchForm::Short,
                            distance,
                            &mut bytes,
                        ) || self.encoder.encode_branch(
                            kind,
                            BranchForm::Long,
                            distance,
                            &mut bytes,
                        );
                        if !reaches {
                            return Err(AssemblyError::BranchOutOfReach(label));
                        }
                    } else {
                        let reaches =
                            self.encoder
                                .encode_branch(kind, BranchForm::Long, 0, &mut bytes);
                        assert!(reaches, "a long branch reaches its own first byte");
                        forward.push((start, bytes.len(), kind, label));
                    }
                }
            }
        }
        let mut patch = Vec::new();
        for (start, end, kind, label) in forward {
            let target = label_offsets[label.index()].ok_or(AssemblyError::UnboundLabel(label))?;
            patch.clear();
            if !self.encoder.encode_branch(
                kind,
                BranchForm::Long,
                distance(start, target)?,
                &mut patch,
            ) {
                return Err(AssemblyError::BranchOutOfReach(label));
            }
            assert_eq!(
                patch.len(),
                end - start,
                "a long branch's size does not depend on its distance"
            );
            bytes[start..end].copy_from_slice(&patch);
        }
        for (start, end, reference, label, displacement) in forward_references {
            let target = label_offsets[label.index()].ok_or(AssemblyError::UnboundLabel(label))?;
            patch.clear();
            if !self.encoder.encode_label_reference(
                reference,
                reference_distance(start, target, displacement)?,
                &mut patch,
            ) {
                return Err(AssemblyError::BranchOutOfReach(label));
            }
            assert_eq!(
                patch.len(),
                end - start,
                "a label reference's size does not depend on its distance"
            );
            bytes[start..end].copy_from_slice(&patch);
        }
        u32::try_from(bytes.len()).map_err(|_| AssemblyError::CodeTooLarge)?;
        Ok(Assembled {
            bytes,
            label_offsets,
            alignment: self.alignment.max(self.encoder.instruction_alignment()),
        })
    }
}

/// The distance from a label reference at `start` to the address
/// `displacement` bytes past `target`, both offsets in the code.
fn reference_distance(start: usize, target: u32, displacement: i32) -> Result<i64, AssemblyError> {
    Ok(distance(start, target)? + i64::from(displacement))
}

/// The distance from a branch at `start` to `target`, both offsets in the
/// code.
fn distance(start: usize, target: u32) -> Result<i64, AssemblyError> {
    let start = i64::try_from(start).map_err(|_| AssemblyError::CodeTooLarge)?;
    Ok(i64::from(target) - start)
}
