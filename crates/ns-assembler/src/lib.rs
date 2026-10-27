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
//! more than one -- so an encoder answers bytes, never one instruction. A
//! branch's distance is known only once every operation before its target
//! is, so branches are encoded in [`Assembler::finish`], in one pass: a
//! backward branch, whose target is already placed, in its shortest form
//! that reaches, a forward one in its longest, patched once its target is
//! placed. A forward branch never shrinks, so no offset placed before it
//! ever moves and one pass suffices.

#![forbid(unsafe_code)]

use std::fmt;

pub mod x64;

pub use x64::X64;

/// A machine register, numbered as the target architecture numbers it in
/// its encodings ([`x64::RAX`] … [`x64::R15`]).
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

/// The condition of a conditional jump, read from the flags the operation
/// before it set. After `compare(left, right)` the signed conditions
/// (`Less` … `GreaterOrEqual`) and the unsigned ones (`Below` …
/// `AboveOrEqual`) compare `left` with `right`; after `multiply_checked`
/// `Overflow` holds when the product did not fit.
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
    /// `target := target + source`, setting the flags.
    Add { target: Register, source: Register },
    /// `target := target - source`, setting the flags.
    Subtract { target: Register, source: Register },
    /// `target := target * source`, signed; `Overflow` when the product
    /// does not fit 64 bits.
    MultiplyChecked { target: Register, source: Register },
    /// `target := target & value` sign-extended.
    AndImmediate { target: Register, value: i32 },
    /// `target := target | value` sign-extended.
    OrImmediate { target: Register, value: i32 },
    /// `target := target << count`, `count` below 64.
    ShiftLeft { target: Register, count: u8 },
    /// `target := target >> count`, the sign copied in, `count` below 64.
    ShiftRightArithmetic { target: Register, count: u8 },
    /// Sets the flags as `left - right` does.
    Compare { left: Register, right: Register },
    /// Sets the flags as `left - right` sign-extended does.
    CompareImmediate { left: Register, right: i32 },
    /// Sets the flags as `left - [right]`, a 64-bit word, does.
    CompareMemory { left: Register, right: Memory },
    /// Sets `Equal` when `left & right` sign-extended is zero.
    TestImmediate { left: Register, right: i32 },
    /// Calls the address in `target`.
    CallRegister { target: Register },
    /// Pushes `source` on the machine stack.
    Push { source: Register },
    /// Pops the machine stack into `target`.
    Pop { target: Register },
    /// Adds `amount` bytes to the stack pointer.
    AddStack { amount: i32 },
    /// Returns to the caller.
    Return,
    /// Stops the program with an illegal-instruction fault.
    Trap,
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
pub trait Encoder: Default {
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
}

/// The code an [`Assembler`] produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Assembled {
    /// The machine code, position-independent: every branch is relative.
    pub bytes: Vec<u8>,
    /// For each label, by [`Label::index`], the offset in `bytes` it was
    /// bound at, `None` for one never bound.
    pub label_offsets: Vec<Option<u32>>,
}

/// Why [`Assembler::finish`] produced no code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssemblyError {
    /// A branch names a label that was never bound.
    UnboundLabel(Label),
    /// A branch's target lies beyond the reach of its longest form.
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
/// a branch still to be sized, or a label's position.
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
}

impl<E: Encoder> Default for Assembler<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: Encoder> Assembler<E> {
    /// An assembler with no code.
    #[must_use]
    pub fn new() -> Self {
        Self {
            encoder: E::default(),
            encoded: Vec::new(),
            items: Vec::new(),
            bound: Vec::new(),
        }
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

    /// `target := source`.
    pub fn move_register(&mut self, target: Register, source: Register) {
        self.emit(Operation::MoveRegister { target, source });
    }

    /// `target := value`.
    pub fn move_immediate(&mut self, target: Register, value: u64) {
        self.emit(Operation::MoveImmediate { target, value });
    }

    /// `target := [source]`, zero-extended from `width`.
    pub fn load(&mut self, width: Width, target: Register, source: Memory) {
        self.emit(Operation::Load {
            width,
            target,
            source,
        });
    }

    /// `[target] := source`'s low `width` bits.
    pub fn store(&mut self, width: Width, target: Memory, source: Register) {
        self.emit(Operation::Store {
            width,
            target,
            source,
        });
    }

    /// The 64-bit word at `target` := `value` sign-extended.
    pub fn store_immediate32(&mut self, target: Memory, value: i32) {
        self.emit(Operation::StoreImmediate32 { target, value });
    }

    /// `target := target + source`, setting the flags.
    pub fn add(&mut self, target: Register, source: Register) {
        self.emit(Operation::Add { target, source });
    }

    /// `target := target - source`, setting the flags.
    pub fn subtract(&mut self, target: Register, source: Register) {
        self.emit(Operation::Subtract { target, source });
    }

    /// `target := target * source`, signed, setting `Overflow` when the
    /// product does not fit 64 bits.
    pub fn multiply_checked(&mut self, target: Register, source: Register) {
        self.emit(Operation::MultiplyChecked { target, source });
    }

    /// `target := target & value` sign-extended.
    pub fn and_immediate(&mut self, target: Register, value: i32) {
        self.emit(Operation::AndImmediate { target, value });
    }

    /// `target := target | value` sign-extended.
    pub fn or_immediate(&mut self, target: Register, value: i32) {
        self.emit(Operation::OrImmediate { target, value });
    }

    /// `target := target << count`; `count` must be below 64.
    pub fn shift_left(&mut self, target: Register, count: u8) {
        self.emit(Operation::ShiftLeft { target, count });
    }

    /// `target := target >> count`, the sign copied in; `count` must be
    /// below 64.
    pub fn shift_right_arithmetic(&mut self, target: Register, count: u8) {
        self.emit(Operation::ShiftRightArithmetic { target, count });
    }

    /// Sets the flags to compare `left` with `right`.
    pub fn compare(&mut self, left: Register, right: Register) {
        self.emit(Operation::Compare { left, right });
    }

    /// Sets the flags to compare `left` with `right` sign-extended.
    pub fn compare_immediate(&mut self, left: Register, right: i32) {
        self.emit(Operation::CompareImmediate { left, right });
    }

    /// Sets the flags to compare `left` with the 64-bit word at `right`.
    pub fn compare_memory(&mut self, left: Register, right: Memory) {
        self.emit(Operation::CompareMemory { left, right });
    }

    /// Sets `Equal` when `left & right` sign-extended is zero, `NotEqual`
    /// otherwise.
    pub fn test_immediate(&mut self, left: Register, right: i32) {
        self.emit(Operation::TestImmediate { left, right });
    }

    /// Jumps to `label`.
    pub fn jump(&mut self, label: Label) {
        self.branch(BranchKind::Unconditional, label);
    }

    /// Jumps to `label` when `condition` holds.
    pub fn jump_if(&mut self, condition: Condition, label: Label) {
        self.branch(BranchKind::Conditional(condition), label);
    }

    /// Calls the address in `target`.
    pub fn call_register(&mut self, target: Register) {
        self.emit(Operation::CallRegister { target });
    }

    /// Pushes `source` on the machine stack.
    pub fn push(&mut self, source: Register) {
        self.emit(Operation::Push { source });
    }

    /// Pops the machine stack into `target`.
    pub fn pop(&mut self, target: Register) {
        self.emit(Operation::Pop { target });
    }

    /// Adds `amount` bytes to the stack pointer.
    pub fn add_stack(&mut self, amount: i32) {
        self.emit(Operation::AddStack { amount });
    }

    /// Returns to the caller.
    pub fn return_(&mut self) {
        self.emit(Operation::Return);
    }

    /// Stops the program with an illegal-instruction fault.
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
        for item in self.items {
            match item {
                Item::Code { start, end } => bytes.extend_from_slice(&self.encoded[start..end]),
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
        u32::try_from(bytes.len()).map_err(|_| AssemblyError::CodeTooLarge)?;
        Ok(Assembled {
            bytes,
            label_offsets,
        })
    }
}

/// The distance from a branch at `start` to `target`, both offsets in the
/// code.
fn distance(start: usize, target: u32) -> Result<i64, AssemblyError> {
    let start = i64::try_from(start).map_err(|_| AssemblyError::CodeTooLarge)?;
    Ok(i64::from(target) - start)
}
