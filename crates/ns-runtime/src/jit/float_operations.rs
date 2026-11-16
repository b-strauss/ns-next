//! Which `Float` operations machine code computes, and what each is
//! (`FloatOperation::of_primitive`): the classification the baseline's float row
//! stub and optimized code's graph, plan and nodes share. It is apart
//! from `floats`, which emits the operations, because the optimizing
//! tier's graph and plan are compiled on every target, a target without
//! a JIT included, and name the operations there
//! (`docs/design/execution.md`, "Floats in machine code").

use ns_assembler::FloatArithmetic;

use crate::builtins::primitive::{FloatPrimitive, IntegerPrimitive, NumberPrimitive, Primitive};

/// A `Float` operation machine code computes: a row of decision 4's
/// groups (`FloatOperation::of_primitive`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FloatOperation {
    /// `+ - * /` of a `Float` receiver with a `Float` or `SmallInteger`
    /// argument, or of a `SmallInteger` receiver with a `Float` argument,
    /// the `SmallInteger` converted to the nearest double.
    Arithmetic {
        operation: FloatArithmetic,
        receiver: FloatReceiver,
    },
    /// `< > <= >= =` of a `Float` receiver with a `Float` or
    /// `SmallInteger` argument, or of a `SmallInteger` receiver with a
    /// `Float` argument, a `SmallInteger` compared exactly.
    Compare {
        comparison: FloatComparison,
        receiver: FloatReceiver,
    },
    SquareRoot,
    Negated,
    Absolute,
    IsNaN,
    IsFinite,
    IsInfinite,
    Floor,
    Ceiling,
    Truncated,
    Rounded,
    AsInteger,
    /// `SmallInteger>>asFloat`.
    IntegerAsFloat,
}

/// What a binary `Float` operation's receiver is: a `Float`'s row, or a
/// `SmallInteger`'s mixed row, which takes only a `Float` argument.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FloatReceiver {
    Float,
    SmallInteger,
}

/// The comparisons of `FloatOperation::Compare`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FloatComparison {
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
    Equal,
}

impl FloatOperation {
    /// The operation `primitive`'s row computes in machine code, or
    /// `None` for a row it leaves to its leaf.
    pub(crate) fn of_primitive(primitive: Primitive) -> Option<FloatOperation> {
        use FloatOperation::{Arithmetic, Compare};
        let arithmetic = |operation, receiver| {
            Some(Arithmetic {
                operation,
                receiver,
            })
        };
        let compare = |comparison, receiver| {
            Some(Compare {
                comparison,
                receiver,
            })
        };
        match primitive {
            Primitive::Float(primitive) => {
                let receiver = FloatReceiver::Float;
                match primitive {
                    FloatPrimitive::Add => arithmetic(FloatArithmetic::Add, receiver),
                    FloatPrimitive::Subtract => arithmetic(FloatArithmetic::Subtract, receiver),
                    FloatPrimitive::Multiply => arithmetic(FloatArithmetic::Multiply, receiver),
                    FloatPrimitive::Divide => arithmetic(FloatArithmetic::Divide, receiver),
                    FloatPrimitive::Less => compare(FloatComparison::Less, receiver),
                    FloatPrimitive::Greater => compare(FloatComparison::Greater, receiver),
                    FloatPrimitive::LessOrEqual => compare(FloatComparison::LessOrEqual, receiver),
                    FloatPrimitive::GreaterOrEqual => {
                        compare(FloatComparison::GreaterOrEqual, receiver)
                    }
                    FloatPrimitive::Equal => compare(FloatComparison::Equal, receiver),
                    FloatPrimitive::SquareRoot => Some(FloatOperation::SquareRoot),
                    FloatPrimitive::Negated => Some(FloatOperation::Negated),
                    FloatPrimitive::Absolute => Some(FloatOperation::Absolute),
                    FloatPrimitive::IsNaN => Some(FloatOperation::IsNaN),
                    FloatPrimitive::IsFinite => Some(FloatOperation::IsFinite),
                    FloatPrimitive::IsInfinite => Some(FloatOperation::IsInfinite),
                    FloatPrimitive::Floor => Some(FloatOperation::Floor),
                    FloatPrimitive::Ceiling => Some(FloatOperation::Ceiling),
                    FloatPrimitive::Truncated => Some(FloatOperation::Truncated),
                    FloatPrimitive::Rounded => Some(FloatOperation::Rounded),
                    FloatPrimitive::AsInteger => Some(FloatOperation::AsInteger),
                    FloatPrimitive::Hash
                    | FloatPrimitive::AsStringFixed
                    | FloatPrimitive::AsStringExponential
                    | FloatPrimitive::AsStringPrecision
                    | FloatPrimitive::Parse
                    | FloatPrimitive::Sine
                    | FloatPrimitive::Cosine
                    | FloatPrimitive::Tangent
                    | FloatPrimitive::ArcSine
                    | FloatPrimitive::ArcCosine
                    | FloatPrimitive::ArcTangent
                    | FloatPrimitive::ArcTangentOf
                    | FloatPrimitive::Exponential
                    | FloatPrimitive::NaturalLogarithm
                    | FloatPrimitive::Logarithm
                    | FloatPrimitive::RaisedTo
                    | FloatPrimitive::PrintString => None,
                }
            }
            Primitive::Integer(primitive) => {
                let receiver = FloatReceiver::SmallInteger;
                match primitive {
                    IntegerPrimitive::Add => arithmetic(FloatArithmetic::Add, receiver),
                    IntegerPrimitive::Subtract => arithmetic(FloatArithmetic::Subtract, receiver),
                    IntegerPrimitive::Multiply => arithmetic(FloatArithmetic::Multiply, receiver),
                    IntegerPrimitive::Less => compare(FloatComparison::Less, receiver),
                    IntegerPrimitive::Greater => compare(FloatComparison::Greater, receiver),
                    IntegerPrimitive::LessOrEqual => {
                        compare(FloatComparison::LessOrEqual, receiver)
                    }
                    IntegerPrimitive::GreaterOrEqual => {
                        compare(FloatComparison::GreaterOrEqual, receiver)
                    }
                    IntegerPrimitive::Equal => compare(FloatComparison::Equal, receiver),
                    IntegerPrimitive::AsFloat => Some(FloatOperation::IntegerAsFloat),
                    IntegerPrimitive::FloorDivide
                    | IntegerPrimitive::Modulo
                    | IntegerPrimitive::BitAnd
                    | IntegerPrimitive::BitOr
                    | IntegerPrimitive::BitXor
                    | IntegerPrimitive::ShiftLeft
                    | IntegerPrimitive::Negated
                    | IntegerPrimitive::Absolute
                    | IntegerPrimitive::PrintString => None,
                }
            }
            Primitive::Number(NumberPrimitive::Divide) => {
                arithmetic(FloatArithmetic::Divide, FloatReceiver::SmallInteger)
            }
            _ => None,
        }
    }

    /// How many arguments the operation takes besides its receiver.
    pub(crate) fn argument_count(self) -> usize {
        match self {
            FloatOperation::Arithmetic { .. } | FloatOperation::Compare { .. } => 1,
            FloatOperation::SquareRoot
            | FloatOperation::Negated
            | FloatOperation::Absolute
            | FloatOperation::IsNaN
            | FloatOperation::IsFinite
            | FloatOperation::IsInfinite
            | FloatOperation::Floor
            | FloatOperation::Ceiling
            | FloatOperation::Truncated
            | FloatOperation::Rounded
            | FloatOperation::AsInteger
            | FloatOperation::IntegerAsFloat => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FloatOperation;
    use crate::builtins::primitive::{MachineCall, RowStub, MIRROR};

    /// `FloatOperation::of_primitive` answers an operation for exactly the rows
    /// machine code computes `Float` operations for: `Float`'s binary
    /// operations, its one-instruction unary ones and its roundings, and
    /// the mixed rows of a `SmallInteger` receiver, `Integer`'s and
    /// `Number`'s `/` -- never a libm function, `hash` or printing.
    #[test]
    fn every_float_row_has_its_operation() {
        let mut found: Vec<&str> = MIRROR
            .iter()
            .filter(|entry| FloatOperation::of_primitive(entry.primitive).is_some())
            .map(|entry| entry.selector)
            .collect();
        found.sort_unstable();
        let mut expected = vec![
            "float:add:ifFail:",
            "float:subtract:ifFail:",
            "float:multiply:ifFail:",
            "float:divide:ifFail:",
            "float:lessThan:ifFail:",
            "float:greaterThan:ifFail:",
            "float:lessOrEqual:ifFail:",
            "float:greaterOrEqual:ifFail:",
            "float:equals:ifFail:",
            "floatSquareRoot:ifFail:",
            "floatNegated:ifFail:",
            "floatAbsolute:ifFail:",
            "floatIsNaN:ifFail:",
            "floatIsFinite:ifFail:",
            "floatIsInfinite:ifFail:",
            "floatFloor:ifFail:",
            "floatCeiling:ifFail:",
            "floatTruncated:ifFail:",
            "floatRounded:ifFail:",
            "floatAsInteger:ifFail:",
            "integer:add:ifFail:",
            "integer:subtract:ifFail:",
            "integer:multiply:ifFail:",
            "integer:lessThan:ifFail:",
            "integer:greaterThan:ifFail:",
            "integer:lessOrEqual:ifFail:",
            "integer:greaterOrEqual:ifFail:",
            "integer:equals:ifFail:",
            "integerAsFloat:ifFail:",
            "number:divide:ifFail:",
        ];
        expected.sort_unstable();
        assert_eq!(found, expected);
        for entry in MIRROR {
            if let Some(operation) = FloatOperation::of_primitive(entry.primitive) {
                assert_eq!(
                    operation.argument_count(),
                    entry.leaf_arity(),
                    "{}",
                    entry.selector
                );
            }
        }
    }

    /// The rows marked with the float stub are exactly those machine
    /// code computes a `Float` operation for, which their stub emits:
    /// `Float`'s binary, unary and rounding operations, and a
    /// `SmallInteger`'s arithmetic, orderings, `=` and `asFloat` with a
    /// `Float`.
    #[test]
    fn every_row_with_a_float_operation_has_the_float_stub() {
        for entry in MIRROR {
            let has_stub = entry.machine_call == MachineCall::Stub(RowStub::Float);
            assert_eq!(
                FloatOperation::of_primitive(entry.primitive).is_some(),
                has_stub,
                "{}",
                entry.selector
            );
        }
    }
}
