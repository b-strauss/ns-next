//! `Float` operations in machine code, written once against the macro
//! assembler's idioms for the baseline's row stubs and optimized code's
//! nodes alike: the rows that compute a `Float` operation in one
//! instruction or a few -- `Float`'s arithmetic, orderings and `=`, its
//! square root, sign, class tests, roundings and `asInteger`, and the
//! mixed rows of a `SmallInteger` receiver with a `Float` argument and
//! `SmallInteger>>asFloat` (`docs/design/execution.md`, "Floats in machine
//! code").
//!
//! Every operation answers bit for bit what its row's leaf answers
//! (`builtins/numbers/float.rs`, `integer.rs`, `tower.rs`) or declines
//! before it has written anything but its own result and spare registers,
//! leaving the case to the leaf: a receiver or an argument the operation
//! does not take, an `=` of two `Float`s that compare unordered (a NaN
//! equals a NaN, which IEEE's comparison does not say), an `asInteger`
//! past the `SmallInteger` range or not finite, and a result to box with
//! eden full. The libm functions, `hash` and printing stay leaves.
//!
//! Which rows these are, and what each computes, is
//! `float_operations`', which compiles without a JIT too, since
//! optimized code's graph and plan name it.

use ns_assembler::{Condition, FloatArithmetic, FloatRegister, Label, Register, Rounding};

use super::float_operations::{FloatComparison, FloatOperation, FloatReceiver};
use super::macro_assembler::{FloatOperand, MacroAssembler};
use crate::heap::{Heap, Immediate, SMALL_INTEGER_MIN};

/// The registers `emit` works in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct FloatRegisters {
    /// The receiver's word; kept.
    pub(crate) receiver: Register,
    /// The argument's word, of a binary operation; kept. It may be the
    /// receiver's register.
    pub(crate) argument: Register,
    /// Where the answer is left.
    pub(crate) result: Register,
    /// A register the operation clobbers.
    pub(crate) spare: Register,
}

/// Emits `operation`: reads `registers.receiver` and, for a binary
/// operation, `registers.argument`, both kept, and leaves the answer in
/// `registers.result`; or jumps to `decline` for every case the row's
/// leaf decides instead, having written nothing -- eden's top
/// included -- but `registers.result` and `registers.spare`. Clobbers
/// `registers.spare`, the float scratch registers
/// (`Roles::float_scratch`), the idiom scratch register and the flags.
pub(crate) fn emit(
    macro_assembler: &mut MacroAssembler,
    operation: FloatOperation,
    registers: FloatRegisters,
    decline: Label,
) {
    let roles = macro_assembler.roles();
    let spare = macro_assembler.temporary(registers.spare);
    let registers = FloatRegisters { spare, ..registers };
    debug_assert!(
        {
            let written = [registers.result, registers.spare, roles.idiom_scratch];
            let read = if operation.argument_count() == 0 {
                vec![registers.receiver]
            } else {
                vec![registers.receiver, registers.argument]
            };
            written[0] != written[1]
                && written[0] != written[2]
                && written[1] != written[2]
                && read.iter().all(|register| !written.contains(register))
        },
        "internal: a float operation writes registers of its own, which hold none of its operands"
    );
    match operation {
        FloatOperation::Arithmetic {
            operation,
            receiver,
        } => arithmetic(macro_assembler, operation, receiver, registers, decline),
        FloatOperation::Compare {
            comparison,
            receiver,
        } => compare(macro_assembler, comparison, receiver, registers, decline),
        FloatOperation::SquareRoot => {
            unary(macro_assembler, registers, decline, |assembler, value| {
                assembler.float_square_root(value, value);
            })
        }
        FloatOperation::Floor => round(
            macro_assembler,
            Rounding::TowardNegative,
            registers,
            decline,
        ),
        FloatOperation::Ceiling => {
            round(
                macro_assembler,
                Rounding::TowardPositive,
                registers,
                decline,
            );
        }
        FloatOperation::Truncated => {
            round(macro_assembler, Rounding::TowardZero, registers, decline)
        }
        FloatOperation::Rounded => rounded(macro_assembler, registers, decline),
        FloatOperation::Negated => sign(macro_assembler, Sign::Flip, registers, decline),
        FloatOperation::Absolute => sign(macro_assembler, Sign::Clear, registers, decline),
        FloatOperation::IsNaN => classify(macro_assembler, Class::NaN, registers, decline),
        FloatOperation::IsFinite => classify(macro_assembler, Class::Finite, registers, decline),
        FloatOperation::IsInfinite => {
            classify(macro_assembler, Class::Infinite, registers, decline)
        }
        FloatOperation::AsInteger => as_integer(macro_assembler, registers, decline),
        FloatOperation::IntegerAsFloat => {
            let [value, ..] = roles.float_scratch;
            small_integer_to_float(macro_assembler, registers.receiver, value, spare, decline);
            macro_assembler.float_result(value, registers.result, spare, decline);
        }
    }
}

/// `target :=` the `SmallInteger` in `value` converted to the nearest
/// double, ties to even, as `integer as f64` converts it, `scratch :=`
/// the integer; or a jump to `decline` for anything else. Clobbers the
/// flags.
fn small_integer_to_float(
    macro_assembler: &mut MacroAssembler,
    value: Register,
    target: FloatRegister,
    scratch: Register,
    decline: Label,
) {
    let layout = Heap::machine_layout();
    let assembler = macro_assembler.assembler();
    assembler.test_immediate(value, layout.small_integer_tag);
    assembler.jump_if(Condition::Equal, decline);
    assembler.move_register(scratch, value);
    assembler.shift_right_arithmetic(scratch, layout.tag_bits);
    assembler.integer_to_float(target, scratch);
}

/// `+ - * /`: `tower::arithmetic_double`'s operands -- a `Float`
/// receiver's argument a `Float` or a `SmallInteger`, a `SmallInteger`
/// receiver's a `Float` -- in IEEE 754's arithmetic, the receiver on the
/// left, as `float::arithmetic` and `tower::float_compute` compute it.
fn arithmetic(
    macro_assembler: &mut MacroAssembler,
    operation: FloatArithmetic,
    receiver: FloatReceiver,
    registers: FloatRegisters,
    decline: Label,
) {
    let [left, right, _] = macro_assembler.roles().float_scratch;
    let spare = registers.spare;
    match receiver {
        FloatReceiver::Float => {
            macro_assembler.load_float(
                registers.receiver,
                left,
                spare,
                FloatOperand::Float,
                decline,
            );
            macro_assembler.load_float(
                registers.argument,
                right,
                spare,
                FloatOperand::FloatOrSmallInteger,
                decline,
            );
        }
        FloatReceiver::SmallInteger => {
            small_integer_to_float(macro_assembler, registers.receiver, left, spare, decline);
            macro_assembler.load_float(
                registers.argument,
                right,
                spare,
                FloatOperand::Float,
                decline,
            );
        }
    }
    macro_assembler
        .assembler()
        .float_arithmetic(operation, left, right);
    macro_assembler.float_result(left, registers.result, spare, decline);
}

/// The orderings and `=`. Two `Float`s compare as IEEE 754 doubles, the
/// orderings by the two conditions false when unordered, the operands
/// swapped for `<` and `<=`, so that a NaN answers `false`; an `=` of two
/// that compare unordered declines, since the leaf's NaN equals a NaN. A
/// `Float` and a `SmallInteger` compare exactly, as `tower::float_order`
/// does, by `compare_mixed`.
fn compare(
    macro_assembler: &mut MacroAssembler,
    comparison: FloatComparison,
    receiver: FloatReceiver,
    registers: FloatRegisters,
    decline: Label,
) {
    let layout = Heap::machine_layout();
    let [float, other, _] = macro_assembler.roles().float_scratch;
    let spare = registers.spare;
    match receiver {
        FloatReceiver::Float => {
            macro_assembler.load_float(
                registers.receiver,
                float,
                spare,
                FloatOperand::Float,
                decline,
            );
            let assembler = macro_assembler.assembler();
            let (mixed, done) = (assembler.label(), assembler.label());
            assembler.test_immediate(registers.argument, layout.small_integer_tag);
            assembler.jump_if(Condition::NotEqual, mixed);
            macro_assembler.load_float(
                registers.argument,
                other,
                spare,
                FloatOperand::Float,
                decline,
            );
            let assembler = macro_assembler.assembler();
            let condition = float_compare(assembler, comparison, float, other);
            if comparison == FloatComparison::Equal {
                assembler.jump_if(Condition::Unordered, decline);
            }
            answer(macro_assembler, condition, registers);
            macro_assembler.assembler().jump(done);
            macro_assembler.assembler().bind(mixed);
            compare_mixed(
                macro_assembler,
                comparison,
                (float, registers.argument),
                true,
                registers,
            );
            macro_assembler.assembler().bind(done);
        }
        FloatReceiver::SmallInteger => {
            let assembler = macro_assembler.assembler();
            assembler.test_immediate(registers.receiver, layout.small_integer_tag);
            assembler.jump_if(Condition::Equal, decline);
            macro_assembler.load_float(
                registers.argument,
                float,
                spare,
                FloatOperand::Float,
                decline,
            );
            compare_mixed(
                macro_assembler,
                comparison,
                (float, registers.receiver),
                false,
                registers,
            );
        }
    }
}

/// Sets the float conditions to compare `left` with `right` by
/// `comparison`, answering the condition that holds when the comparison
/// does: `FloatGreater` and `FloatGreaterOrEqual`, never true unordered,
/// with the operands swapped for `<` and `<=`; `Equal` for `=`, defined
/// only once `Unordered` was tested false.
fn float_compare(
    assembler: &mut ns_assembler::Assembler<super::macro_assembler::Encoding>,
    comparison: FloatComparison,
    left: FloatRegister,
    right: FloatRegister,
) -> Condition {
    match comparison {
        FloatComparison::Less => {
            assembler.float_compare(right, left);
            Condition::FloatGreater
        }
        FloatComparison::LessOrEqual => {
            assembler.float_compare(right, left);
            Condition::FloatGreaterOrEqual
        }
        FloatComparison::Greater => {
            assembler.float_compare(left, right);
            Condition::FloatGreater
        }
        FloatComparison::GreaterOrEqual => {
            assembler.float_compare(left, right);
            Condition::FloatGreaterOrEqual
        }
        FloatComparison::Equal => {
            assembler.float_compare(left, right);
            Condition::Equal
        }
    }
}

/// The integer condition of `comparison`, signed.
fn integer_condition(comparison: FloatComparison) -> Condition {
    match comparison {
        FloatComparison::Less => Condition::Less,
        FloatComparison::Greater => Condition::Greater,
        FloatComparison::LessOrEqual => Condition::LessOrEqual,
        FloatComparison::GreaterOrEqual => Condition::GreaterOrEqual,
        FloatComparison::Equal => Condition::Equal,
    }
}

/// `registers.result :=` the Boolean of `condition` on the flags as they
/// are. Keeps the flags; clobbers `registers.spare`.
fn answer(macro_assembler: &mut MacroAssembler, condition: Condition, registers: FloatRegisters) {
    let assembler = macro_assembler.assembler();
    assembler.move_immediate(registers.result, Immediate::boolean(false).bits());
    assembler.move_immediate(registers.spare, Immediate::boolean(true).bits());
    assembler.move_if(condition, registers.result, registers.spare);
}

/// A `Float` and a `SmallInteger` compared exactly, as
/// `tower::float_order` and `tower::equal` compare them, for every
/// `SmallInteger`, by Cog's sequence: the `Float` in `float` against the
/// integer in `integer`'s word converted to the nearest double. A NaN
/// answers `false`. Unless the two doubles are equal, they decide: the
/// conversion is monotonic, so a `Float` below the converted integer lies
/// below the integer, and one that equals the integer converts to itself.
/// Equal, the `Float` is integral and within the `SmallInteger` range, so
/// its truncation is exactly it, and the integers decide. `float_is_left`
/// says which side of the comparison the `Float` is on.
fn compare_mixed(
    macro_assembler: &mut MacroAssembler,
    comparison: FloatComparison,
    (float, integer): (FloatRegister, Register),
    float_is_left: bool,
    registers: FloatRegisters,
) {
    let roles = macro_assembler.roles();
    let [_, _, converted] = roles.float_scratch;
    let (untagged, truncated) = (registers.spare, roles.idiom_scratch);
    let (false_word, true_word) = (
        Immediate::boolean(false).bits(),
        Immediate::boolean(true).bits(),
    );
    let layout = Heap::machine_layout();
    let assembler = macro_assembler.assembler();
    let (exact, done) = (assembler.label(), assembler.label());
    assembler.move_register(untagged, integer);
    assembler.shift_right_arithmetic(untagged, layout.tag_bits);
    assembler.integer_to_float(converted, untagged);
    let (left, right) = if float_is_left {
        (float, converted)
    } else {
        (converted, float)
    };
    let condition = float_compare(assembler, comparison, left, right);
    assembler.move_immediate(registers.result, false_word);
    assembler.jump_if(Condition::Unordered, done);
    assembler.jump_if(Condition::Equal, exact);
    // Unequal doubles: they decide, and an `=` is false.
    if comparison != FloatComparison::Equal {
        assembler.move_immediate(truncated, true_word);
        assembler.move_if(condition, registers.result, truncated);
    }
    assembler.jump(done);
    assembler.bind(exact);
    assembler.float_to_integer_truncated(truncated, float);
    if float_is_left {
        assembler.compare(truncated, untagged);
    } else {
        assembler.compare(untagged, truncated);
    }
    assembler.move_immediate(untagged, true_word);
    assembler.move_if(integer_condition(comparison), registers.result, untagged);
    assembler.bind(done);
}

/// A unary operation on a `Float` receiver answering a `Float`: the
/// receiver's double in the first float scratch register, `operation`
/// emitted on it there, and the double left there the answer.
fn unary(
    macro_assembler: &mut MacroAssembler,
    registers: FloatRegisters,
    decline: Label,
    operation: impl FnOnce(
        &mut ns_assembler::Assembler<super::macro_assembler::Encoding>,
        FloatRegister,
    ),
) {
    let [value, ..] = macro_assembler.roles().float_scratch;
    macro_assembler.load_float(
        registers.receiver,
        value,
        registers.spare,
        FloatOperand::Float,
        decline,
    );
    operation(macro_assembler.assembler(), value);
    macro_assembler.float_result(value, registers.result, registers.spare, decline);
}

/// `floor`, `ceiling` and `truncated`: the receiver rounded to an
/// integral `Float` in `rounding`'s direction, as `f64::floor`, `ceil`
/// and `trunc` round it, a NaN and an infinity themselves.
fn round(
    macro_assembler: &mut MacroAssembler,
    rounding: Rounding,
    registers: FloatRegisters,
    decline: Label,
) {
    unary(macro_assembler, registers, decline, |assembler, value| {
        assembler.float_round(rounding, value, value);
    });
}

/// `rounded`, half away from zero as `f64::round`: `t` the receiver
/// truncated, then `t + 1` where the receiver lies half or more above
/// `t`, `t - 1` where half or more below, else `t` -- exact, since the
/// receiver less its truncation is its fraction, and `t + 1` and `t - 1`
/// arise only below 2^52, where they are doubles. `-0.4` answers `-0.0`,
/// `t`'s sign, as `f64::round` does.
fn rounded(macro_assembler: &mut MacroAssembler, registers: FloatRegisters, decline: Label) {
    let [value, truncated, constant] = macro_assembler.roles().float_scratch;
    let spare = registers.spare;
    macro_assembler.load_float(
        registers.receiver,
        value,
        spare,
        FloatOperand::Float,
        decline,
    );
    let assembler = macro_assembler.assembler();
    let (round_up, round_down, result) = (assembler.label(), assembler.label(), assembler.label());
    let load_constant = |assembler: &mut ns_assembler::Assembler<_>, double: f64| {
        assembler.move_immediate(spare, double.to_bits());
        assembler.move_to_float(constant, spare);
    };
    assembler.float_round(Rounding::TowardZero, truncated, value);
    // The fraction, its sign the receiver's.
    assembler.float_arithmetic(FloatArithmetic::Subtract, value, truncated);
    load_constant(assembler, 0.5);
    assembler.float_compare(value, constant);
    assembler.jump_if(Condition::FloatGreaterOrEqual, round_up);
    load_constant(assembler, -0.5);
    assembler.float_compare(constant, value);
    assembler.jump_if(Condition::FloatGreaterOrEqual, round_down);
    assembler.jump(result);
    assembler.bind(round_up);
    load_constant(assembler, 1.0);
    assembler.float_arithmetic(FloatArithmetic::Add, truncated, constant);
    assembler.jump(result);
    assembler.bind(round_down);
    load_constant(assembler, 1.0);
    assembler.float_arithmetic(FloatArithmetic::Subtract, truncated, constant);
    assembler.bind(result);
    macro_assembler.float_result(truncated, registers.result, spare, decline);
}

/// What `negated` and `abs` do to the sign bit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sign {
    Flip,
    Clear,
}

/// `negated` and `abs`: the sign bit flipped or cleared, nothing else
/// changed, a NaN's payload kept, as `-x` and `f64::abs` do. A
/// `SmallFloat64` keeps its sign in the word's lowest bit above the tag,
/// so its word changes there and stays a `SmallFloat64`, the range being
/// symmetric; a boxed `Float`'s bits, rotated left by one to bring the
/// sign to bit 0, change there, and stay outside the range, so the answer
/// is boxed.
fn sign(
    macro_assembler: &mut MacroAssembler,
    change: Sign,
    registers: FloatRegisters,
    decline: Label,
) {
    let layout = Heap::machine_layout();
    let [value, ..] = macro_assembler.roles().float_scratch;
    let spare = registers.spare;
    let sign_bit = 1_i32 << layout.tag_bits;
    let assembler = macro_assembler.assembler();
    let (small, done) = (assembler.label(), assembler.label());
    assembler.test_immediate(registers.receiver, layout.small_float_tag);
    assembler.jump_if(Condition::NotEqual, small);
    macro_assembler.load_float(
        registers.receiver,
        value,
        spare,
        FloatOperand::Float,
        decline,
    );
    let assembler = macro_assembler.assembler();
    assembler.move_from_float(spare, value);
    assembler.rotate_left(spare, 1);
    match change {
        Sign::Flip => assembler.xor_immediate(spare, 1),
        Sign::Clear => assembler.and_immediate(spare, !1),
    }
    assembler.rotate_right(spare, 1);
    assembler.move_to_float(value, spare);
    macro_assembler.box_float(value, registers.result, spare, decline);
    let assembler = macro_assembler.assembler();
    assembler.jump(done);
    assembler.bind(small);
    assembler.move_register(registers.result, registers.receiver);
    match change {
        Sign::Flip => assembler.xor_immediate(registers.result, sign_bit),
        Sign::Clear => assembler.and_immediate(registers.result, !sign_bit),
    }
    assembler.bind(done);
}

/// What `isNaN`, `isFinite` and `isInfinite` ask.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    NaN,
    Finite,
    Infinite,
}

/// The bits of an infinity shifted left by one, the sign shifted out:
/// the exponent all ones and no mantissa. Every double's bits so shifted
/// lie below it when it is finite, above it when it is a NaN.
const INFINITY_SHIFTED: u64 = f64::INFINITY.to_bits() << 1;

/// `isNaN`, `isFinite` and `isInfinite`. A `SmallFloat64` is finite and no
/// NaN: its answer is a constant. A boxed `Float`'s bits, the sign
/// shifted out, compared with an infinity's (`INFINITY_SHIFTED`).
fn classify(
    macro_assembler: &mut MacroAssembler,
    class: Class,
    registers: FloatRegisters,
    decline: Label,
) {
    let layout = Heap::machine_layout();
    let roles = macro_assembler.roles();
    let [value, ..] = roles.float_scratch;
    let (spare, infinity) = (registers.spare, roles.idiom_scratch);
    let (condition, small_answer) = match class {
        Class::NaN => (Condition::Above, false),
        Class::Finite => (Condition::Below, true),
        Class::Infinite => (Condition::Equal, false),
    };
    let assembler = macro_assembler.assembler();
    let (small, done) = (assembler.label(), assembler.label());
    assembler.test_immediate(registers.receiver, layout.small_float_tag);
    assembler.jump_if(Condition::NotEqual, small);
    macro_assembler.load_float(
        registers.receiver,
        value,
        spare,
        FloatOperand::Float,
        decline,
    );
    let assembler = macro_assembler.assembler();
    assembler.move_from_float(spare, value);
    assembler.shift_left(spare, 1);
    assembler.move_immediate(infinity, INFINITY_SHIFTED);
    assembler.compare(spare, infinity);
    answer(macro_assembler, condition, registers);
    let assembler = macro_assembler.assembler();
    assembler.jump(done);
    assembler.bind(small);
    assembler.move_immediate(registers.result, Immediate::boolean(small_answer).bits());
    assembler.bind(done);
}

/// The bits of 2^60, one past the largest `SmallInteger`, shifted left by
/// one, the sign shifted out: a double whose bits so shifted lie below it
/// is finite and of a magnitude below 2^60, so its truncation is a
/// `SmallInteger`. `-2^60`, the one other double that truncates to one,
/// is left to the leaf.
const SMALL_INTEGER_BOUND_SHIFTED: u64 = {
    #[expect(
        clippy::cast_precision_loss,
        reason = "-2^60 converts exactly: a power of two"
    )]
    let bound = -(SMALL_INTEGER_MIN as f64);
    bound.to_bits() << 1
};

/// `asInteger`: the receiver truncated toward zero, a `SmallInteger`, as
/// `ArbitraryInteger::from_f64_truncated` and `Mutator::new_integer`
/// answer it; declining for a receiver not finite, whose error the leaf
/// raises, or of a magnitude of 2^60 or more, whose integer may be a
/// `LargeInteger` the leaf makes.
fn as_integer(macro_assembler: &mut MacroAssembler, registers: FloatRegisters, decline: Label) {
    let layout = Heap::machine_layout();
    let roles = macro_assembler.roles();
    let [value, ..] = roles.float_scratch;
    let (spare, bound) = (registers.spare, roles.idiom_scratch);
    macro_assembler.load_float(
        registers.receiver,
        value,
        spare,
        FloatOperand::Float,
        decline,
    );
    let assembler = macro_assembler.assembler();
    assembler.move_from_float(spare, value);
    assembler.shift_left(spare, 1);
    assembler.move_immediate(bound, SMALL_INTEGER_BOUND_SHIFTED);
    assembler.compare(spare, bound);
    assembler.jump_if(Condition::AboveOrEqual, decline);
    assembler.float_to_integer_truncated(registers.result, value);
    assembler.shift_left(registers.result, layout.tag_bits);
    assembler.or_immediate(registers.result, layout.small_integer_tag);
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use ns_assembler::FloatArithmetic;

    use super::{emit, FloatRegisters};
    use crate::builtins::numbers::tower::{self, Operand};
    use crate::heap::machine_words::MachineWord;
    use crate::heap::{is_small_float_bits, Heap, Immediate, SMALL_INTEGER_MAX, SMALL_INTEGER_MIN};
    use crate::jit::float_operations::{FloatComparison, FloatOperation, FloatReceiver};
    use crate::jit::macro_assembler::{Abi, Roles};
    use crate::jit::stubs::testing::Machine;
    use crate::jit::stubs::{BlockWord, ExitKind, MachineExit};

    /// The dispatch id the tests' `Float` application guards on.
    const FLOAT_GUARD: u64 = 0x1234;
    /// The tests' `Float` class word, an object's.
    const FLOAT_CLASS: u64 = 0x5670;
    /// What eden held before an operation, so that a test sees which
    /// words it wrote.
    const UNWRITTEN: u64 = 0xDEAD_BEEF;

    /// A boxed `Float`'s header, as `Heap::set_float_class` makes it.
    fn float_header() -> u64 {
        let layout = Heap::machine_layout();
        FLOAT_GUARD
            | u64::from(layout.sixty_four_bit_format.cast_unsigned())
            | (2 << layout.word_count_shift)
    }

    /// An operand: a double, a `SmallInteger`, or a word that is neither.
    #[derive(Clone, Copy, Debug)]
    enum Number {
        Float(f64),
        Integer(i64),
        Other(u64),
    }

    /// What a leaf answers, or the decline machine code takes instead.
    #[derive(Clone, Copy, Debug)]
    enum Expected {
        Float(f64),
        Boolean(bool),
        Integer(i64),
        Decline,
    }

    /// The objects the operands name, kept alive at their addresses,
    /// which are their words, since the tests' object space starts at 0.
    struct Objects {
        #[expect(
            clippy::vec_box,
            reason = "each object keeps its address, which is its word, as the vector grows"
        )]
        objects: Vec<Box<[u64; 3]>>,
    }

    impl Objects {
        fn word(&mut self, number: Number) -> u64 {
            match number {
                Number::Float(double) => match Immediate::small_float(double) {
                    Some(immediate) => immediate.bits(),
                    None => self.object([float_header(), FLOAT_CLASS, double.to_bits()]),
                },
                Number::Integer(integer) => Immediate::small_integer(integer)
                    .expect("a SmallInteger")
                    .bits(),
                Number::Other(word) => word,
            }
        }

        fn object(&mut self, words: [u64; 3]) -> u64 {
            let object = Box::new(words);
            let address = object.as_ptr().expose_provenance() as u64;
            self.objects.push(object);
            address
        }
    }

    /// Doubles at every edge: the zeros, halves and their neighbours,
    /// 2^53 and 2^60 and their neighbours, the `SmallFloat64` range's
    /// ends and the doubles just outside, subnormals, the infinities and
    /// NaN.
    fn doubles() -> Vec<f64> {
        let two_to_the = |exponent: i32| 2f64.powi(exponent);
        let mut doubles = vec![
            0.0,
            1.0,
            0.5,
            0.49999999999999994,
            0.1,
            1.5,
            2.5,
            3.7,
            123_456.789,
            1e20,
            two_to_the(52) + 0.5,
            two_to_the(53),
            two_to_the(53) + 2.0,
            two_to_the(59),
            two_to_the(60),
            two_to_the(60) - 128.0,
            two_to_the(61),
            1e300,
            1e-300,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
            f64::MAX,
            f64::INFINITY,
            f64::NAN,
            // The largest `SmallFloat64` and the double past it.
            f64::from_bits((1151 << 52) | ((1 << 52) - 1)),
            f64::from_bits(1152 << 52),
            // The smallest, and the exponent-896 double without a
            // mantissa below it, which is boxed.
            f64::from_bits((896 << 52) | 1),
            f64::from_bits(896 << 52),
            f64::from_bits(897 << 52),
        ];
        let negated: Vec<f64> = doubles.iter().map(|double| -double).collect();
        doubles.extend(negated);
        doubles
    }

    fn integers() -> Vec<i64> {
        let two_to_the = |exponent: u32| 1_i64 << exponent;
        vec![
            0,
            1,
            -1,
            3,
            -7,
            1_000_000_000_000_000,
            two_to_the(53),
            two_to_the(53) + 1,
            -(two_to_the(53) + 1),
            two_to_the(59) + 1,
            -(two_to_the(59) + 1),
            two_to_the(60) - 1,
            SMALL_INTEGER_MAX,
            SMALL_INTEGER_MIN,
        ]
    }

    /// Every operand: the doubles, the integers, and words of neither --
    /// `nil`, `true`, a `LargeInteger`-like object of the same format
    /// under another guard, and a `Float`-guarded object of another
    /// format, as `Float new` is.
    fn operands(objects: &mut Objects) -> Vec<Number> {
        let layout = Heap::machine_layout();
        let other_guard = 0x999;
        let large_header = other_guard
            | u64::from(layout.sixty_four_bit_format.cast_unsigned())
            | (2 << layout.word_count_shift);
        let instance_header = FLOAT_GUARD | (2 << layout.word_count_shift);
        let mut operands: Vec<Number> = doubles().into_iter().map(Number::Float).collect();
        operands.extend(integers().into_iter().map(Number::Integer));
        operands.push(Number::Other(Immediate::NIL.bits()));
        operands.push(Number::Other(Immediate::boolean(true).bits()));
        operands.push(Number::Other(objects.object([large_header, 0x7770, 5])));
        operands.push(Number::Other(objects.object([
            instance_header,
            FLOAT_CLASS,
            0,
        ])));
        operands
    }

    /// The leaf's answer, by the leaves' own arithmetic and the tower's
    /// comparison, or the decline the operation takes.
    fn expected(operation: FloatOperation, receiver: Number, argument: Number) -> Expected {
        let black_box = std::hint::black_box;
        let holds = |comparison: FloatComparison, left: &Operand, right: &Operand| match comparison
        {
            FloatComparison::Equal => tower::equal(left, right),
            _ => tower::order(left, right).is_some_and(|order| match comparison {
                FloatComparison::Less => order == Ordering::Less,
                FloatComparison::Greater => order == Ordering::Greater,
                FloatComparison::LessOrEqual => order != Ordering::Greater,
                FloatComparison::GreaterOrEqual => order != Ordering::Less,
                FloatComparison::Equal => unreachable!(),
            }),
        };
        let compute = |operation: FloatArithmetic, left: f64, right: f64| {
            let (left, right) = (black_box(left), black_box(right));
            match operation {
                FloatArithmetic::Add => left + right,
                FloatArithmetic::Subtract => left - right,
                FloatArithmetic::Multiply => left * right,
                FloatArithmetic::Divide => left / right,
            }
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "the leaves convert a SmallInteger to the nearest double"
        )]
        let to_double = |integer: i64| integer as f64;
        match (operation, receiver, argument) {
            (
                FloatOperation::Arithmetic {
                    operation,
                    receiver: FloatReceiver::Float,
                },
                Number::Float(left),
                Number::Float(right),
            ) => Expected::Float(compute(operation, left, right)),
            (
                FloatOperation::Arithmetic {
                    operation,
                    receiver: FloatReceiver::Float,
                },
                Number::Float(left),
                Number::Integer(right),
            ) => Expected::Float(compute(operation, left, to_double(right))),
            (
                FloatOperation::Arithmetic {
                    operation,
                    receiver: FloatReceiver::SmallInteger,
                },
                Number::Integer(left),
                Number::Float(right),
            ) => Expected::Float(compute(operation, to_double(left), right)),
            (
                FloatOperation::Compare {
                    comparison,
                    receiver: FloatReceiver::Float,
                },
                Number::Float(left),
                Number::Float(right),
            ) => {
                if comparison == FloatComparison::Equal && (left.is_nan() || right.is_nan()) {
                    Expected::Decline
                } else {
                    Expected::Boolean(holds(
                        comparison,
                        &Operand::Float(left),
                        &Operand::Float(right),
                    ))
                }
            }
            (
                FloatOperation::Compare {
                    comparison,
                    receiver: FloatReceiver::Float,
                },
                Number::Float(left),
                Number::Integer(right),
            ) => Expected::Boolean(holds(
                comparison,
                &Operand::Float(left),
                &Operand::Small(right),
            )),
            (
                FloatOperation::Compare {
                    comparison,
                    receiver: FloatReceiver::SmallInteger,
                },
                Number::Integer(left),
                Number::Float(right),
            ) => Expected::Boolean(holds(
                comparison,
                &Operand::Small(left),
                &Operand::Float(right),
            )),
            (FloatOperation::SquareRoot, Number::Float(value), _) => {
                Expected::Float(black_box(value).sqrt())
            }
            (FloatOperation::Negated, Number::Float(value), _) => {
                Expected::Float(-black_box(value))
            }
            (FloatOperation::Absolute, Number::Float(value), _) => {
                Expected::Float(black_box(value).abs())
            }
            (FloatOperation::IsNaN, Number::Float(value), _) => Expected::Boolean(value.is_nan()),
            (FloatOperation::IsFinite, Number::Float(value), _) => {
                Expected::Boolean(value.is_finite())
            }
            (FloatOperation::IsInfinite, Number::Float(value), _) => {
                Expected::Boolean(value.is_infinite())
            }
            (FloatOperation::Floor, Number::Float(value), _) => {
                Expected::Float(black_box(value).floor())
            }
            (FloatOperation::Ceiling, Number::Float(value), _) => {
                Expected::Float(black_box(value).ceil())
            }
            (FloatOperation::Truncated, Number::Float(value), _) => {
                Expected::Float(black_box(value).trunc())
            }
            (FloatOperation::Rounded, Number::Float(value), _) => {
                Expected::Float(black_box(value).round())
            }
            (FloatOperation::AsInteger, Number::Float(value), _) => {
                if value.is_finite() && value.abs() < -to_double(SMALL_INTEGER_MIN) {
                    Expected::Integer(truncated(value))
                } else {
                    Expected::Decline
                }
            }
            (FloatOperation::IntegerAsFloat, Number::Integer(value), _) => {
                Expected::Float(to_double(value))
            }
            _ => Expected::Decline,
        }
    }

    /// `value` truncated, a finite double of a magnitude below 2^60.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a finite double below 2^60 truncates into an i64"
    )]
    fn truncated(value: f64) -> i64 {
        value.trunc() as i64
    }

    /// One operation's body, run on operand after operand: the receiver
    /// in frame register 0, the argument in 1, the answer stored into 2,
    /// the operands' registers into 3 and 4, and an exit at 1, or an exit
    /// at 2 for the decline; eden a few
    /// words, its limit `limit_words` past its start.
    struct Harness {
        machine: Machine,
        body: ns_memory::CodeOffset,
        eden: Vec<u64>,
        cursor: Box<[u64; 2]>,
        limit_words: u64,
    }

    const ANSWERED: MachineExit = MachineExit::Reexecute { program_counter: 1 };
    const DECLINED: MachineExit = MachineExit::Reexecute { program_counter: 2 };

    impl Harness {
        fn new(operation: FloatOperation, limit_words: u64) -> Harness {
            let roles = Roles::of_abi(Abi::host());
            let [receiver, argument, result, spare] =
                std::array::from_fn(|index| roles.scratch[index]);
            let eden = vec![UNWRITTEN; 8];
            let cursor = Box::new([0, 0]);
            let mut machine = Machine::new();
            machine.set_block_word(
                BlockWord::EdenCursor,
                cursor.as_ptr().expose_provenance() as u64,
            );
            let layout = Heap::machine_layout();
            let guard_index = usize::try_from(layout.small_float_index).expect("small");
            machine.set_machine_word(MachineWord::ImmediateGuard(guard_index), FLOAT_GUARD);
            machine.set_machine_word(MachineWord::FloatHeader, float_header());
            machine.set_machine_word(MachineWord::FloatClass, FLOAT_CLASS);
            let body = machine.body(|macro_assembler| {
                let decline = macro_assembler.assembler().label();
                macro_assembler.load_frame_register(receiver, 0);
                macro_assembler.load_frame_register(argument, 1);
                let registers = FloatRegisters {
                    receiver,
                    argument,
                    result,
                    spare,
                };
                emit(macro_assembler, operation, registers, decline);
                macro_assembler.store_frame_register(2, result);
                macro_assembler.store_frame_register(3, receiver);
                macro_assembler.store_frame_register(4, argument);
                macro_assembler.exit(ExitKind::Reexecute, 1);
                macro_assembler.assembler().bind(decline);
                macro_assembler.exit(ExitKind::Reexecute, 2);
            });
            Harness {
                machine,
                body,
                eden,
                cursor,
                limit_words,
            }
        }

        fn start(&self) -> u64 {
            self.eden.as_ptr().expose_provenance() as u64
        }

        /// Runs the body on `receiver` and `argument`, answering the exit
        /// and the answer's word.
        fn run(&mut self, receiver: u64, argument: u64) -> (MachineExit, u64) {
            self.eden.fill(UNWRITTEN);
            let start = self.start();
            *self.cursor = [start, start + 8 * self.limit_words];
            let mut frame = [receiver, argument, UNWRITTEN, receiver, argument, 0];
            let (exit, _) = self
                .machine
                .run(self.body, &mut frame, 0, false, &mut (), &[]);
            assert_eq!(
                frame[3..5],
                [receiver, argument],
                "the operands' registers are kept"
            );
            (exit, frame[2])
        }

        /// Whether `word`, an answer, is the value of `double`: its
        /// `SmallFloat64` word in the range, else a boxed `Float` just
        /// placed at eden's start with `FloatHeader`, `FloatClass` and its
        /// bits, eden's top moved past it.
        fn holds_double(&self, word: u64, double: f64) -> bool {
            match Immediate::small_float(double) {
                Some(immediate) => word == immediate.bits() && self.cursor[0] == self.start(),
                None => {
                    !is_small_float_bits(double.to_bits())
                        && word == self.start()
                        && self.eden[..4]
                            == [float_header(), FLOAT_CLASS, double.to_bits(), UNWRITTEN]
                        && self.cursor[0] == self.start() + 24
                }
            }
        }
    }

    /// Every operation, binary ones on every pair of operands, answers
    /// what its leaf answers bit for bit -- the `SmallFloat64` word, or a
    /// boxed `Float` in eden -- or declines exactly where the leaf
    /// decides, writing nothing.
    #[test]
    fn every_float_operation_answers_its_leaf_s_value_or_declines() {
        let mut objects = Objects {
            objects: Vec::new(),
        };
        let operands = operands(&mut objects);
        let words: Vec<u64> = operands
            .iter()
            .map(|operand| objects.word(*operand))
            .collect();
        let mut operations = Vec::new();
        for receiver in [FloatReceiver::Float, FloatReceiver::SmallInteger] {
            for operation in [
                FloatArithmetic::Add,
                FloatArithmetic::Subtract,
                FloatArithmetic::Multiply,
                FloatArithmetic::Divide,
            ] {
                operations.push(FloatOperation::Arithmetic {
                    operation,
                    receiver,
                });
            }
            for comparison in [
                FloatComparison::Less,
                FloatComparison::Greater,
                FloatComparison::LessOrEqual,
                FloatComparison::GreaterOrEqual,
                FloatComparison::Equal,
            ] {
                operations.push(FloatOperation::Compare {
                    comparison,
                    receiver,
                });
            }
        }
        operations.extend([
            FloatOperation::SquareRoot,
            FloatOperation::Negated,
            FloatOperation::Absolute,
            FloatOperation::IsNaN,
            FloatOperation::IsFinite,
            FloatOperation::IsInfinite,
            FloatOperation::Floor,
            FloatOperation::Ceiling,
            FloatOperation::Truncated,
            FloatOperation::Rounded,
            FloatOperation::AsInteger,
            FloatOperation::IntegerAsFloat,
        ]);
        let nil = Immediate::NIL.bits();
        for operation in operations {
            let mut harness = Harness::new(operation, 8);
            let arguments: Vec<(Number, u64)> = if operation.argument_count() == 0 {
                vec![(Number::Other(nil), nil)]
            } else {
                operands
                    .iter()
                    .copied()
                    .zip(words.iter().copied())
                    .collect()
            };
            for (receiver, receiver_word) in operands.iter().zip(&words) {
                for (argument, argument_word) in &arguments {
                    let (exit, answer) = harness.run(*receiver_word, *argument_word);
                    let case = format!("{operation:?} {receiver:?} {argument:?}");
                    match expected(operation, *receiver, *argument) {
                        Expected::Decline => {
                            assert_eq!(exit, DECLINED, "{case}");
                            assert_eq!(harness.cursor[0], harness.start(), "{case}");
                        }
                        Expected::Float(double) => {
                            assert_eq!(exit, ANSWERED, "{case}");
                            assert!(
                                harness.holds_double(answer, double),
                                "{case}: {answer:#x} for {double:?} ({:#x})",
                                double.to_bits()
                            );
                        }
                        Expected::Boolean(boolean) => {
                            assert_eq!(
                                (exit, answer),
                                (ANSWERED, Immediate::boolean(boolean).bits()),
                                "{case}"
                            );
                        }
                        Expected::Integer(integer) => {
                            let word = Immediate::small_integer(integer).expect("small").bits();
                            assert_eq!((exit, answer), (ANSWERED, word), "{case}");
                        }
                    }
                }
            }
        }
    }

    /// A result outside the `SmallFloat64` range with eden full declines,
    /// eden's top and its words untouched, so that the leaf allocates; one
    /// inside it needs no room.
    #[test]
    fn a_float_boxed_with_eden_full_declines_having_written_nothing() {
        let mut objects = Objects {
            objects: Vec::new(),
        };
        for operation in [
            FloatOperation::Arithmetic {
                operation: FloatArithmetic::Multiply,
                receiver: FloatReceiver::Float,
            },
            FloatOperation::Negated,
        ] {
            let mut harness = Harness::new(operation, 2);
            let huge = objects.word(Number::Float(1e300));
            let small = objects.word(Number::Float(1.5));
            let (exit, _) = harness.run(huge, small);
            assert_eq!(exit, DECLINED, "{operation:?}");
            assert_eq!(harness.cursor[0], harness.start());
            assert!(harness.eden.iter().all(|word| *word == UNWRITTEN));
            let (exit, answer) = harness.run(small, small);
            assert_eq!(exit, ANSWERED);
            let expected = if operation == FloatOperation::Negated {
                -1.5
            } else {
                2.25
            };
            assert!(harness.holds_double(answer, expected), "{operation:?}");
        }
    }

    /// The tag and the decode are `Value::small_float`'s and
    /// `Value::kind`'s for every double: `+ 0.0`'s answer for a stream of
    /// bit patterns over the whole range, edges included, is the word of
    /// the same double, immediate exactly where `is_small_float_bits`
    /// says.
    #[test]
    fn the_small_float_encoding_is_the_value_s_for_every_double() {
        let mut objects = Objects {
            objects: Vec::new(),
        };
        let mut harness = Harness::new(
            FloatOperation::Arithmetic {
                operation: FloatArithmetic::Add,
                receiver: FloatReceiver::Float,
            },
            8,
        );
        let zero = objects.word(Number::Float(-0.0));
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut patterns: Vec<u64> = (0..20_000)
            .map(|_| {
                // xorshift64: a deterministic spread over every exponent.
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            })
            .collect();
        for exponent in [0, 1, 895, 896, 897, 1150, 1151, 1152, 2046, 2047] {
            for mantissa in [0, 1, (1 << 52) - 1] {
                for sign in [0, 1 << 63] {
                    patterns.push(sign | (exponent << 52) | mantissa);
                }
            }
        }
        for bits in patterns {
            let double = f64::from_bits(bits);
            if double.is_nan() {
                continue;
            }
            let word = objects.word(Number::Float(double));
            let (exit, answer) = harness.run(word, zero);
            assert_eq!(exit, ANSWERED, "{bits:#x}");
            assert!(harness.holds_double(answer, double), "{bits:#x}");
        }
    }
}
