//! The enumerated primitives: every native operation of the builtin classes,
//! named rather than selected by its selector's text, so that a lookup can
//! bind one and a later JIT tier can recognize it and emit it inline
//! (`docs/design/dispatch.md`, "Primitives").

use ns_syntax::ast;

use crate::hash::IdentityMap;
use crate::mixin::{MethodEntry, PrimitiveMethod};
use crate::Symbol;

/// One native operation. Grouped by the builtin family whose module executes
/// it, so each module matches its own group exhaustively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primitive {
    Object(ObjectPrimitive),
    Value(ValuePrimitive),
    Behavior(BehaviorPrimitive),
    Integer(IntegerPrimitive),
    LargeInteger(LargeIntegerPrimitive),
    AnyInteger(AnyIntegerPrimitive),
    Float(FloatPrimitive),
    /// `Fraction`'s leaves: its two parts, and its making in normal form
    /// (`builtins::numbers::fraction`).
    Fraction(FractionPrimitive),
    /// `Number`'s own leaves, over every representation: the tower
    /// (`builtins::numbers::tower`).
    Number(NumberPrimitive),
    String(StringPrimitive),
    Symbol(SymbolPrimitive),
    Collection(CollectionPrimitive),
    /// `ByteArray`'s leaves (`builtins::bytes`).
    Bytes(BytePrimitive),
    /// An element of a kind read or written on a `ByteArray`: the typed
    /// arrays' and `DataView`'s accessors (`builtins::elements`).
    Element(ElementAccess),
    Block(BlockPrimitive),
    Platform(PlatformPrimitive),
    Activation(ActivationPrimitive),
    Message(MessagePrimitive),
    /// `moduleInstanceNamed:`, `shippedModuleClassNamed:` and
    /// `recordShippedModule:`: a shipped module's one instance, and its
    /// build on first demand (`builtins::module`).
    Module(ModulePrimitive),
    /// §6.3.2's pull future, made by `Future computing:` and forced by its
    /// `doesNotUnderstand:` (`builtins::future`).
    Future(FuturePrimitive),
    /// `WeakArray`'s leaves and `garbageCollect` (`builtins::weak`).
    Weak(WeakPrimitive),
    /// What the shipped mirrors module reads of classes, mixins, methods
    /// and objects (`builtins::reflection`).
    Reflection(ReflectionPrimitive),
    /// A leaf reached as a send to the VM mirror: a row of `MIRROR`.
    Mirror(MirrorPrimitive),
}

impl Primitive {
    /// Whether this leaf answers `false` for an operand of the wrong kind
    /// instead of declining: identity and the equality leaves. Each of them
    /// declines instead when that answer is `false` and an operand is a
    /// future (`interpreter::future::declines_for_a_future`, which checks
    /// in a debug build that its caller is one of these), so its caller's
    /// failure path sees through it, since a future answers here for its
    /// value (`docs/design/object-model.md`, "Where Rust sees a future").
    /// Every variant is named, so a new leaf does not compile until someone
    /// decides.
    ///
    /// Every leaf marked `false` either declines or raises on an operand of
    /// the wrong kind -- which the failure path covers -- or never needs to
    /// see through one: it has no operand but its own subject, or it stores,
    /// forwards or ignores the operand (`ElementAtPut`'s element, `Closure`'s
    /// `Value` arguments, `Message`'s `SendTo` receiver, `ReturnValue`'s
    /// value). The three that take a block, or a text they render, and would
    /// otherwise answer as though a future were a value of its own --
    /// `Enumerate`'s and the loops' block, `Terminate`'s arguments --
    /// decline for a future instead.
    pub(crate) fn answers_false_to_a_stranger(self) -> bool {
        match self {
            Primitive::Object(ObjectPrimitive::Identical | ObjectPrimitive::Equal) => true,
            Primitive::Object(
                ObjectPrimitive::Class | ObjectPrimitive::Hash | ObjectPrimitive::PrintString,
            ) => false,
            Primitive::Value(ValuePrimitive::SameAs) => true,
            Primitive::Value(
                ValuePrimitive::Hash | ValuePrimitive::SlotCount | ValuePrimitive::SlotAt,
            ) => false,
            Primitive::Behavior(
                // Declines, rather than answering `false`, for an argument
                // that is no class object -- a future among them -- and for
                // a receiver that is none, so the failure path already sees
                // through a future and it needs no `declines_for_a_future`.
                BehaviorPrimitive::SharesApplication
                | BehaviorPrimitive::Name
                | BehaviorPrimitive::ThisClass
                | BehaviorPrimitive::Enclosing
                | BehaviorPrimitive::Superclass,
            ) => false,
            Primitive::Integer(IntegerPrimitive::Equal) => true,
            Primitive::Integer(
                IntegerPrimitive::Add
                | IntegerPrimitive::Subtract
                | IntegerPrimitive::Multiply
                | IntegerPrimitive::FloorDivide
                | IntegerPrimitive::Modulo
                | IntegerPrimitive::BitAnd
                | IntegerPrimitive::BitOr
                | IntegerPrimitive::BitXor
                | IntegerPrimitive::ShiftLeft
                | IntegerPrimitive::Less
                | IntegerPrimitive::Greater
                | IntegerPrimitive::LessOrEqual
                | IntegerPrimitive::GreaterOrEqual
                | IntegerPrimitive::Negated
                | IntegerPrimitive::Absolute
                | IntegerPrimitive::AsFloat
                | IntegerPrimitive::PrintString,
            ) => false,
            Primitive::AnyInteger(
                AnyIntegerPrimitive::Gcd
                | AnyIntegerPrimitive::ShiftRight
                | AnyIntegerPrimitive::DigitAt
                | AnyIntegerPrimitive::DigitLength
                | AnyIntegerPrimitive::AsStringRadix
                | AnyIntegerPrimitive::Parse,
            ) => false,
            Primitive::LargeInteger(LargeIntegerPrimitive::Equal) => true,
            Primitive::LargeInteger(
                LargeIntegerPrimitive::Add
                | LargeIntegerPrimitive::Subtract
                | LargeIntegerPrimitive::Multiply
                | LargeIntegerPrimitive::FloorDivide
                | LargeIntegerPrimitive::Modulo
                | LargeIntegerPrimitive::BitAnd
                | LargeIntegerPrimitive::BitOr
                | LargeIntegerPrimitive::BitXor
                | LargeIntegerPrimitive::ShiftLeft
                | LargeIntegerPrimitive::Less
                | LargeIntegerPrimitive::Greater
                | LargeIntegerPrimitive::LessOrEqual
                | LargeIntegerPrimitive::GreaterOrEqual
                | LargeIntegerPrimitive::Hash
                | LargeIntegerPrimitive::Negated
                | LargeIntegerPrimitive::Absolute
                | LargeIntegerPrimitive::AsFloat
                | LargeIntegerPrimitive::PrintString,
            ) => false,
            Primitive::Float(FloatPrimitive::Equal) => true,
            Primitive::Float(
                FloatPrimitive::Add
                | FloatPrimitive::Subtract
                | FloatPrimitive::Multiply
                | FloatPrimitive::Divide
                | FloatPrimitive::Less
                | FloatPrimitive::Greater
                | FloatPrimitive::LessOrEqual
                | FloatPrimitive::GreaterOrEqual
                | FloatPrimitive::Hash
                | FloatPrimitive::AsStringFixed
                | FloatPrimitive::AsStringExponential
                | FloatPrimitive::AsStringPrecision
                | FloatPrimitive::Parse
                | FloatPrimitive::SquareRoot
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
                | FloatPrimitive::Floor
                | FloatPrimitive::Ceiling
                | FloatPrimitive::Rounded
                | FloatPrimitive::Truncated
                | FloatPrimitive::Negated
                | FloatPrimitive::Absolute
                | FloatPrimitive::IsNaN
                | FloatPrimitive::IsFinite
                | FloatPrimitive::IsInfinite
                | FloatPrimitive::AsInteger
                | FloatPrimitive::PrintString,
            ) => false,
            Primitive::Fraction(
                FractionPrimitive::Numerator
                | FractionPrimitive::Denominator
                | FractionPrimitive::Make
                | FractionPrimitive::MakeReduced,
            ) => false,
            Primitive::Number(NumberPrimitive::Equal) => true,
            Primitive::Number(
                NumberPrimitive::Less
                | NumberPrimitive::Greater
                | NumberPrimitive::LessOrEqual
                | NumberPrimitive::GreaterOrEqual
                | NumberPrimitive::Hash
                | NumberPrimitive::Add
                | NumberPrimitive::Subtract
                | NumberPrimitive::Multiply
                | NumberPrimitive::Divide
                | NumberPrimitive::FloorDivide
                | NumberPrimitive::Modulo
                | NumberPrimitive::Quotient
                | NumberPrimitive::Remainder
                | NumberPrimitive::AsFloat,
            ) => false,
            Primitive::String(StringPrimitive::Equal) => true,
            Primitive::String(
                StringPrimitive::Size
                | StringPrimitive::Concatenate
                | StringPrimitive::Hash
                | StringPrimitive::AsSymbol
                | StringPrimitive::ElementAt
                | StringPrimitive::CopyRange
                | StringPrimitive::FromCodePoint
                | StringPrimitive::IndexOf
                | StringPrimitive::LastIndexOf
                | StringPrimitive::FromCodePoints
                | StringPrimitive::PrintString
                | StringPrimitive::AsUtf8
                | StringPrimitive::FromUtf8
                | StringPrimitive::Utf8Size,
            ) => false,
            Primitive::Symbol(SymbolPrimitive::Equal) => true,
            Primitive::Symbol(
                SymbolPrimitive::AsString | SymbolPrimitive::Hash | SymbolPrimitive::PrintString,
            ) => false,
            Primitive::Collection(
                CollectionPrimitive::Enumerate
                | CollectionPrimitive::Size
                | CollectionPrimitive::ElementAt
                | CollectionPrimitive::ElementAtPut
                | CollectionPrimitive::Allocate
                | CollectionPrimitive::CopyRange
                | CollectionPrimitive::Replace,
            ) => false,
            Primitive::Bytes(
                BytePrimitive::Allocate
                | BytePrimitive::Fits
                | BytePrimitive::Size
                | BytePrimitive::ElementAt
                | BytePrimitive::ElementAtPut
                | BytePrimitive::CopyRange
                | BytePrimitive::Replace
                | BytePrimitive::Concatenate
                | BytePrimitive::IndexOf
                | BytePrimitive::LastIndexOf
                | BytePrimitive::StartsWith
                | BytePrimitive::EndsWith
                | BytePrimitive::WriteCodePoint,
            ) => false,
            // A write stores its element, and a value of the wrong kind is
            // an error rather than `false`.
            Primitive::Element(_) => false,
            Primitive::Block(
                BlockPrimitive::Value
                | BlockPrimitive::ArgumentCount
                | BlockPrimitive::WhileTrue
                | BlockPrimitive::WhileFalse,
            ) => false,
            Primitive::Platform(
                PlatformPrimitive::Show
                | PlatformPrimitive::NewLine
                | PlatformPrimitive::MonotonicNanoseconds,
            ) => false,
            Primitive::Activation(ActivationPrimitive::InheritsMixinOf) => true,
            Primitive::Activation(
                ActivationPrimitive::Current
                | ActivationPrimitive::Sender
                | ActivationPrimitive::MethodName
                | ActivationPrimitive::IsActiveHandler
                | ActivationPrimitive::HandlerClass
                | ActivationPrimitive::HandlerBlock
                | ActivationPrimitive::SetHandlerActive
                | ActivationPrimitive::ReturnValue
                | ActivationPrimitive::Restart
                | ActivationPrimitive::Terminate,
            ) => false,
            Primitive::Message(MessagePrimitive::SendTo) => false,
            Primitive::Module(
                ModulePrimitive::InstanceNamed
                | ModulePrimitive::ClassNamed
                | ModulePrimitive::Record,
            ) => false,
            Primitive::Future(FuturePrimitive::Value | FuturePrimitive::Made) => false,
            Primitive::Weak(
                WeakPrimitive::Allocate
                | WeakPrimitive::Size
                | WeakPrimitive::ElementAt
                | WeakPrimitive::ElementAtPut
                | WeakPrimitive::Collect
                | WeakPrimitive::EphemeronKey
                | WeakPrimitive::EphemeronSetKey
                | WeakPrimitive::EphemeronValue
                | WeakPrimitive::EphemeronSetValue
                | WeakPrimitive::EphemeronFinalizer
                | WeakPrimitive::EphemeronSetFinalizer,
            ) => false,
            // A future is no value, but it may stand for one, which a weak
            // structure must refuse as a key (§6.3.2: a future is
            // transparent).
            Primitive::Weak(WeakPrimitive::IsValue) => true,
            // Declines, rather than answering `false`, for a subject or an
            // argument of the wrong kind -- a future among them -- as
            // `Behavior`'s leaves do. `SlotNamesOf` never declines: it
            // reads a future as the object it is, whose levels are all the
            // kernel's, and answers no slot. None needs to see through a
            // future, and none does (`interpreter::future::after_a_failed_leaf`):
            // a mirror reflects the future itself, so `Identical` answers
            // `false` for a future and another object without declining.
            Primitive::Reflection(
                ReflectionPrimitive::MixinOf
                | ReflectionPrimitive::IsMetaclass
                | ReflectionPrimitive::EnclosingObjectOf
                | ReflectionPrimitive::SlotNamesOf
                | ReflectionPrimitive::SlotNamed
                | ReflectionPrimitive::MixinName
                | ReflectionPrimitive::MixinIsMeta
                | ReflectionPrimitive::MixinEnclosing
                | ReflectionPrimitive::MixinInstanceSide
                | ReflectionPrimitive::MixinClassSide
                | ReflectionPrimitive::MixinSlots
                | ReflectionPrimitive::MixinMethods
                | ReflectionPrimitive::MixinNested
                | ReflectionPrimitive::MixinAccess
                | ReflectionPrimitive::MethodSource
                | ReflectionPrimitive::MixinSource
                | ReflectionPrimitive::MixinIsShipped
                | ReflectionPrimitive::Identical
                | ReflectionPrimitive::SymbolText
                | ReflectionPrimitive::ForwardIdentity,
            ) => false,
            // It reaches its own primitive's row through `execute_sent`,
            // whose failure path looks for a future among that row's
            // operands.
            Primitive::Mirror(_) => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectPrimitive {
    Class,
    Identical,
    Equal,
    Hash,
    PrintString,
}

/// §3.1.1's value objects: the equality and hash `Value` derives from an
/// instance's state, and the two reflective leaves that read that state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValuePrimitive {
    SameAs,
    Hash,
    SlotCount,
    SlotAt,
}

/// §3.2's `Class` and `Metaclass` forward to these; nothing else about a
/// class object is answered at the base level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BehaviorPrimitive {
    /// A class's name, its mixin's, as a `String`; declines for a metaclass.
    Name,
    /// A metaclass's class; declines for anything else.
    ThisClass,
    /// Whether two class objects apply one mixin application; declines
    /// when either is no class object. The first of the three leaves the
    /// Newspeak side of a value class object's `=` reads its state through.
    SharesApplication,
    /// A class object's enclosing object, `nil` for a module definition.
    Enclosing,
    /// A class object's superclass class object, `nil` for `Top`.
    Superclass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegerPrimitive {
    Add,
    Subtract,
    Multiply,
    FloorDivide,
    Modulo,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
    Equal,
    Negated,
    Absolute,
    AsFloat,
    PrintString,
}

/// The leaves that take an integer of either representation (`Gcd`,
/// `ShiftRight`, `DigitAt`, `DigitLength`, `AsStringRadix`), or the `Integer`
/// class itself (`Parse`). A family of its own so that `IntegerPrimitive`
/// keeps meaning "the receiver is a `SmallInteger`", which a JIT tier emits
/// inline, and `LargeInteger` inherits these methods from `Integer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnyIntegerPrimitive {
    Gcd,
    ShiftRight,
    DigitAt,
    DigitLength,
    AsStringRadix,
    Parse,
}

/// `LargeInteger`'s leaves: `Integer`'s protocol for a receiver whose
/// shape is `ObjectShape::LargeInteger`. A family of its own rather than
/// `Integer`'s leaves widened, so `IntegerPrimitive` keeps meaning "the
/// receiver is a `SmallInteger`", which is what a JIT tier emits inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LargeIntegerPrimitive {
    Add,
    Subtract,
    Multiply,
    FloorDivide,
    Modulo,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
    Equal,
    Hash,
    Negated,
    Absolute,
    AsFloat,
    PrintString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FractionPrimitive {
    /// `numerator`: a `Fraction`'s numerator.
    Numerator,
    /// `denominator`: a `Fraction`'s denominator.
    Denominator,
    /// `Fraction class>>numerator:denominator:`: any two integers, the
    /// denominator not zero, made normal.
    Make,
    /// `Fraction class>>reducedNumerator:denominator:`: a pair already in
    /// normal form, checked.
    MakeReduced,
}

/// `Number`'s comparisons, `=` and `hash`, by exact value across every
/// representation, its arithmetic -- exact unless a `Float` takes part --
/// and `asFloat`; `Fraction` inherits them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberPrimitive {
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
    Equal,
    Hash,
    Add,
    Subtract,
    Multiply,
    Divide,
    FloorDivide,
    Modulo,
    Quotient,
    Remainder,
    AsFloat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatPrimitive {
    Add,
    Subtract,
    Multiply,
    Divide,
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
    Equal,
    Hash,
    AsStringFixed,
    AsStringExponential,
    AsStringPrecision,
    Parse,
    SquareRoot,
    Sine,
    Cosine,
    Tangent,
    ArcSine,
    ArcCosine,
    ArcTangent,
    ArcTangentOf,
    Exponential,
    NaturalLogarithm,
    Logarithm,
    RaisedTo,
    Floor,
    Ceiling,
    Rounded,
    Truncated,
    Negated,
    Absolute,
    IsNaN,
    IsFinite,
    IsInfinite,
    AsInteger,
    PrintString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringPrimitive {
    Size,
    Concatenate,
    Equal,
    Hash,
    AsSymbol,
    ElementAt,
    CopyRange,
    /// `String with: code`, on `String`'s class side: its subject is the
    /// class.
    FromCodePoint,
    /// `indexOf:startingAt:`: the code-point index of the first occurrence
    /// at or after a start, or 0.
    IndexOf,
    /// `lastIndexOf:startingAt:`: the code-point index of the last
    /// occurrence at or before a start, or 0.
    LastIndexOf,
    /// `String withAll: codePoints`, on `String`'s class side: its subject
    /// is the class.
    FromCodePoints,
    PrintString,
    /// `asUtf8`: a new ByteArray of the UTF-8 bytes.
    AsUtf8,
    /// `String fromUtf8: bytes from: start to: stop`, on `String`'s class
    /// side: its subject is the class.
    FromUtf8,
    /// `utf8Size`: the number of UTF-8 bytes.
    Utf8Size,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolPrimitive {
    AsString,
    Equal,
    Hash,
    PrintString,
}

/// `Array`'s primitives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionPrimitive {
    Enumerate,
    Size,
    ElementAt,
    ElementAtPut,
    /// `Array new: size`, on `Array`'s class side.
    Allocate,
    /// `copyFrom:to:`: a new Array of a range.
    CopyRange,
    /// `replaceFrom:to:with:startingAt:` from another Array, or the same.
    Replace,
}

/// `ByteArray`'s primitives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytePrimitive {
    /// `ByteArray new: size`, on `ByteArray`'s class side.
    Allocate,
    /// Whether `ByteArray new: size` would allocate: what a shipped
    /// module that sizes a ByteArray for a message of its own asks before
    /// it allocates, so that it refuses a size too large under the
    /// selector the program sent.
    Fits,
    Size,
    ElementAt,
    ElementAtPut,
    /// `copyFrom:to:`: a new ByteArray of a range.
    CopyRange,
    /// `replaceFrom:to:with:startingAt:` from a ByteArray, the same one
    /// included, or a String's UTF-8 bytes.
    Replace,
    /// `,` with a ByteArray or a String's UTF-8 bytes.
    Concatenate,
    /// `indexOf:startingAt:` of a byte or a subsequence.
    IndexOf,
    /// `lastIndexOf:startingAt:` of a byte or a subsequence.
    LastIndexOf,
    StartsWith,
    EndsWith,
    /// `at:putCodePoint:`: one code point's UTF-8 bytes, stored from an
    /// index.
    WriteCodePoint,
}

/// `WeakArray`'s primitives, the full collection a program asks for,
/// `Ephemeron`'s fields, and the question a weak structure asks of a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeakPrimitive {
    /// `WeakArray new: size`, on `WeakArray`'s class side.
    Allocate,
    Size,
    ElementAt,
    ElementAtPut,
    /// `garbageCollect`: a full collection, served at the next safepoint.
    Collect,
    /// Whether an object is a value (§3.1.1), which `Ephemeron>>key:`
    /// refuses as a key.
    IsValue,
    /// `Ephemeron>>key`: the key, its first field.
    EphemeronKey,
    /// `Ephemeron>>key:`: stores the key, whose refusal of a value is
    /// `Kernel.ns`'s.
    EphemeronSetKey,
    /// `Ephemeron>>value`: the value, its second field.
    EphemeronValue,
    /// `Ephemeron>>value:`: stores the value.
    EphemeronSetValue,
    /// `Ephemeron>>finalizer`: the finalizer, its third field.
    EphemeronFinalizer,
    /// `Ephemeron>>finalizer:`: stores the finalizer.
    EphemeronSetFinalizer,
}

/// The ten element kinds of the typed arrays and `DataView`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ElementKind {
    Int8,
    Uint8,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Int64,
    Uint64,
    Float32,
    Float64,
}

impl ElementKind {
    /// The number of bytes an element of the kind takes.
    pub const fn width(self) -> usize {
        match self {
            ElementKind::Int8 | ElementKind::Uint8 => 1,
            ElementKind::Int16 | ElementKind::Uint16 => 2,
            ElementKind::Int32 | ElementKind::Uint32 | ElementKind::Float32 => 4,
            ElementKind::Int64 | ElementKind::Uint64 | ElementKind::Float64 => 8,
        }
    }

    /// The least and the greatest Integer an element of an integer kind
    /// holds; `None` for a float kind.
    pub const fn integer_range(self) -> Option<(i128, i128)> {
        match self {
            ElementKind::Int8 => Some((i8::MIN as i128, i8::MAX as i128)),
            ElementKind::Uint8 => Some((0, u8::MAX as i128)),
            ElementKind::Int16 => Some((i16::MIN as i128, i16::MAX as i128)),
            ElementKind::Uint16 => Some((0, u16::MAX as i128)),
            ElementKind::Int32 => Some((i32::MIN as i128, i32::MAX as i128)),
            ElementKind::Uint32 => Some((0, u32::MAX as i128)),
            ElementKind::Int64 => Some((i64::MIN as i128, i64::MAX as i128)),
            ElementKind::Uint64 => Some((0, u64::MAX as i128)),
            ElementKind::Float32 | ElementKind::Float64 => None,
        }
    }

    /// The kind whose discriminant is `discriminant`; `None` past the ten.
    const fn from_discriminant(discriminant: u8) -> Option<ElementKind> {
        Some(match discriminant {
            0 => ElementKind::Int8,
            1 => ElementKind::Uint8,
            2 => ElementKind::Int16,
            3 => ElementKind::Uint16,
            4 => ElementKind::Int32,
            5 => ElementKind::Uint32,
            6 => ElementKind::Int64,
            7 => ElementKind::Uint64,
            8 => ElementKind::Float32,
            9 => ElementKind::Float64,
            _ => return None,
        })
    }
}

/// One access to a `ByteArray` at a width: bits 0-3 the kind, bit 4
/// big-endian, bit 5 a 1-based byte position (else a 1-based element
/// index), bit 6 a write. One byte, so that `Primitive` stays two
/// (`resolve.rs`): the typed arrays' and `DataView`'s 54 rows are one leaf
/// over these four choices rather than a variant each.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ElementAccess(u8);

impl ElementAccess {
    const KIND: u8 = 0b1111;
    const BIG_ENDIAN: u8 = 1 << 4;
    const BYTE_POSITION: u8 = 1 << 5;
    const WRITE: u8 = 1 << 6;

    pub const fn new(
        kind: ElementKind,
        big_endian: bool,
        byte_position: bool,
        write: bool,
    ) -> ElementAccess {
        let mut bits = kind as u8;
        if big_endian {
            bits |= ElementAccess::BIG_ENDIAN;
        }
        if byte_position {
            bits |= ElementAccess::BYTE_POSITION;
        }
        if write {
            bits |= ElementAccess::WRITE;
        }
        ElementAccess(bits)
    }

    pub fn kind(self) -> ElementKind {
        match ElementKind::from_discriminant(self.0 & ElementAccess::KIND) {
            Some(kind) => kind,
            // `new` is the only way to make one, and it stores a kind.
            None => unreachable!("an ElementAccess holds one of the ten kinds"),
        }
    }

    pub fn big_endian(self) -> bool {
        self.0 & ElementAccess::BIG_ENDIAN != 0
    }

    pub fn byte_position(self) -> bool {
        self.0 & ElementAccess::BYTE_POSITION != 0
    }

    pub fn write(self) -> bool {
        self.0 & ElementAccess::WRITE != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockPrimitive {
    Value,
    ArgumentCount,
    WhileTrue,
    WhileFalse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformPrimitive {
    Show,
    NewLine,
    MonotonicNanoseconds,
}

/// The activation leaves `Kernel.ns`'s `Exception` is built on
/// (`docs/design/core-library.md`, "Exceptions"): reading the
/// stack, the `on:do:` activations' marked registers, and the three ways an
/// exception leaves -- a return from, or a restart of, an activation, and
/// the termination an unhandled one ends in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationPrimitive {
    Current,
    Sender,
    /// The name `Object>>subclassResponsibility` reports: the class that
    /// declares the activation's method and its selector, `nil` for an
    /// activation running no method.
    MethodName,
    IsActiveHandler,
    HandlerClass,
    HandlerBlock,
    SetHandlerActive,
    ReturnValue,
    Restart,
    InheritsMixinOf,
    Terminate,
}

/// `Message>>sendTo:`'s leaf: the message sent to a receiver as an
/// ordinary public send (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePrimitive {
    SendTo,
}

/// The module table's leaves (`docs/design/core-library.md`, "The module
/// table and the boot").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModulePrimitive {
    /// A recorded shipped module's one instance.
    InstanceNamed,
    /// The root class of a lazy shipped module not yet recorded, built
    /// once and kept pending until its instance is recorded.
    ClassNamed,
    /// Records an instance of a pending root class as its module's one
    /// instance.
    Record,
}

/// The two leaves of the futures (`docs/design/object-model.md`,
/// "Simultaneous slots"): `FutureImplementation`'s and `Future`'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FuturePrimitive {
    /// The future's value, forcing it first when it is unresolved.
    Value,
    /// Notes that `FutureImplementation`'s factory made a future, which
    /// turns on the runtime's looking for one (`Mutator::futures_made`).
    Made,
}

/// The leaves the shipped mirrors module reads structure and state
/// through (`builtins::reflection`). A class leaf's subject is a class or
/// a metaclass, a mixin leaf's a reified mixin (`Mutator::reified_mixin`);
/// each declines for any other. A value is answered only through a level
/// whose mixin is not shipped, and never when it is withheld
/// (`Mutator::is_withheld`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectionPrimitive {
    /// The reified mixin a class object's application applies.
    MixinOf,
    /// Whether a class object is a metaclass.
    IsMetaclass,
    /// A class object's enclosing object, `nil` for a module's class;
    /// declined for a shipped level and a withheld value.
    EnclosingObjectOf,
    /// The names of an object's visible slots, each once, superclass level
    /// first: under each name the slot the nearest level declaring it
    /// declares, the name left out when that declaration is lazy or on a
    /// shipped level; never declines.
    SlotNamesOf,
    /// The value of an object's visible slot of a name, as `SlotNamesOf`
    /// decides it; declined when the nearest declaration is lazy or on a
    /// shipped level -- never reading a farther slot it shadows -- and for
    /// a withheld value.
    SlotNamed,
    /// A mixin's name, a class side's `<instance side's name> class`.
    MixinName,
    /// Whether a mixin is a class side.
    MixinIsMeta,
    /// The mixin a mixin is declared in, a class side's that of its
    /// instance side; `nil` at a module's root.
    MixinEnclosing,
    /// A mixin's instance side, itself for an instance side.
    MixinInstanceSide,
    /// A mixin's class side, declared or synthesized, itself for a class
    /// side.
    MixinClassSide,
    /// A mixin's own slot declarations.
    MixinSlots,
    /// A mixin's own declared methods.
    MixinMethods,
    /// A mixin's nested mixins.
    MixinNested,
    /// The access of a mixin's accessor, a class side's that of its
    /// instance side.
    MixinAccess,
    /// A declared method's source text.
    MethodSource,
    /// A class declaration's source text.
    MixinSource,
    /// Whether a mixin is shipped, a class side's whether its instance
    /// side is.
    MixinIsShipped,
    /// Whether two objects are one, by identity, a future the future
    /// itself: never declined, so no future is forced for it.
    Identical,
    /// A Symbol's text, a new String; declined for anything else, a
    /// future included, which it never forces.
    SymbolText,
    /// `become:` as an eager bulk forward: every object of one `Array`
    /// forwarded to the object of another at the same index, answering
    /// `nil` (`heap::world::forward_identity`). Declined, nothing changed,
    /// unless both are `Array`s every element of which the leaf's lock
    /// admits (`builtins/reflection.rs`'s `may_be_forwarded`: a
    /// `FixedPointers` instance of a class that is no builtin and whose
    /// mixin is not shipped, no reified mixin, no withheld value), and on
    /// every `ForwardRefusal`.
    ForwardIdentity,
}

/// A row of `MIRROR`. A `u16`, so the table may hold 65,536 rows: two bytes
/// more than a `u8` makes `Primitive` four bytes, which fit in the padding
/// every structure holding one already had, so none grows (`Target`,
/// `CacheEntry`, `GlobalEntry`, `Resolved` and `MirrorEntry`; the size of
/// `Resolved` is pinned by `interpreter::resolve`'s
/// `a_resolution_stays_the_size_the_hot_path_was_measured_at`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorPrimitive(u16);

impl MirrorPrimitive {
    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// The row at `index` of `MIRROR`, for the embedded shipped code, whose
    /// bytes are the binary's own.
    pub(crate) fn from_index(index: u16) -> MirrorPrimitive {
        assert!(
            usize::from(index) < MIRROR.len(),
            "internal: no mirror row {index}"
        );
        MirrorPrimitive(index)
    }
}

/// One leaf, as the VM mirror answers it. The mirror selector's arguments
/// are the subject (the kernel method's `self`), the leaf's own arguments,
/// and the failure block, so it takes two more than `reported` does.
#[derive(Debug, Clone, Copy)]
pub struct MirrorEntry {
    /// What `Kernel.ns` sends to the mirror.
    pub selector: &'static str,
    /// The kernel selector the leaf's error texts name -- the one the
    /// program sent, so no text a program sees changes by moving behind the
    /// mirror.
    pub reported: &'static str,
    /// The leaf itself; never a `Primitive::Mirror`.
    pub primitive: Primitive,
    /// How many arguments `reported` takes, counted once when the table is
    /// built rather than on every mirror send that checks its arguments.
    leaf_arity: usize,
}

impl MirrorEntry {
    /// How many arguments the reported selector takes: one for a binary
    /// selector, one per colon for a keyword selector, none for a unary one.
    pub fn leaf_arity(&self) -> usize {
        self.leaf_arity
    }
}

/// `MirrorEntry::leaf_arity`'s count, as a byte scan so the table can run it
/// at compile time. Every reported selector is ASCII, so a byte that is not
/// an ASCII letter opens a binary selector.
const fn selector_arity(selector: &str) -> usize {
    let bytes = selector.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return 1;
    }
    let mut colons = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b':' {
            colons += 1;
        }
        index += 1;
    }
    colons
}

const fn mirror(
    selector: &'static str,
    reported: &'static str,
    primitive: Primitive,
) -> MirrorEntry {
    MirrorEntry {
        selector,
        reported,
        primitive,
        leaf_arity: selector_arity(reported),
    }
}

/// The complete leaf table the VM mirror answers (`docs/design/core-library.md`,
/// "The VM mirror is the whole Rust boundary"); a row's index is its
/// position here, which `MirrorPrimitive` carries. The rows here are
/// `Array`'s.
pub const MIRROR: &[MirrorEntry] = &[
    mirror(
        "objectClass:ifFail:",
        "class",
        Primitive::Object(ObjectPrimitive::Class),
    ),
    mirror(
        "object:identicalTo:ifFail:",
        "==",
        Primitive::Object(ObjectPrimitive::Identical),
    ),
    mirror(
        "object:equals:ifFail:",
        "=",
        Primitive::Object(ObjectPrimitive::Equal),
    ),
    mirror(
        "objectHash:ifFail:",
        "hash",
        Primitive::Object(ObjectPrimitive::Hash),
    ),
    mirror(
        "objectPrintString:ifFail:",
        "printString",
        Primitive::Object(ObjectPrimitive::PrintString),
    ),
    mirror(
        "value:sameAs:ifFail:",
        "=",
        Primitive::Value(ValuePrimitive::SameAs),
    ),
    mirror(
        "valueHash:ifFail:",
        "hash",
        Primitive::Value(ValuePrimitive::Hash),
    ),
    mirror(
        "valueSlotCount:ifFail:",
        "slotCount",
        Primitive::Value(ValuePrimitive::SlotCount),
    ),
    mirror(
        "value:slotAt:ifFail:",
        "slotAt:",
        Primitive::Value(ValuePrimitive::SlotAt),
    ),
    mirror(
        "integer:add:ifFail:",
        "+",
        Primitive::Integer(IntegerPrimitive::Add),
    ),
    mirror(
        "integer:subtract:ifFail:",
        "-",
        Primitive::Integer(IntegerPrimitive::Subtract),
    ),
    mirror(
        "integer:multiply:ifFail:",
        "*",
        Primitive::Integer(IntegerPrimitive::Multiply),
    ),
    mirror(
        "integer:floorDivide:ifFail:",
        "//",
        Primitive::Integer(IntegerPrimitive::FloorDivide),
    ),
    mirror(
        "integer:lessThan:ifFail:",
        "<",
        Primitive::Integer(IntegerPrimitive::Less),
    ),
    mirror(
        "integer:greaterThan:ifFail:",
        ">",
        Primitive::Integer(IntegerPrimitive::Greater),
    ),
    mirror(
        "integer:lessOrEqual:ifFail:",
        "<=",
        Primitive::Integer(IntegerPrimitive::LessOrEqual),
    ),
    mirror(
        "integer:greaterOrEqual:ifFail:",
        ">=",
        Primitive::Integer(IntegerPrimitive::GreaterOrEqual),
    ),
    mirror(
        "integer:equals:ifFail:",
        "=",
        Primitive::Integer(IntegerPrimitive::Equal),
    ),
    mirror(
        "integerNegated:ifFail:",
        "negated",
        Primitive::Integer(IntegerPrimitive::Negated),
    ),
    mirror(
        "integerAbsolute:ifFail:",
        "abs",
        Primitive::Integer(IntegerPrimitive::Absolute),
    ),
    mirror(
        "integerPrintString:ifFail:",
        "printString",
        Primitive::Integer(IntegerPrimitive::PrintString),
    ),
    mirror(
        "integerAsString:ifFail:",
        "asString",
        Primitive::Integer(IntegerPrimitive::PrintString),
    ),
    mirror(
        "largeInteger:add:ifFail:",
        "+",
        Primitive::LargeInteger(LargeIntegerPrimitive::Add),
    ),
    mirror(
        "largeInteger:subtract:ifFail:",
        "-",
        Primitive::LargeInteger(LargeIntegerPrimitive::Subtract),
    ),
    mirror(
        "largeInteger:multiply:ifFail:",
        "*",
        Primitive::LargeInteger(LargeIntegerPrimitive::Multiply),
    ),
    mirror(
        "largeInteger:floorDivide:ifFail:",
        "//",
        Primitive::LargeInteger(LargeIntegerPrimitive::FloorDivide),
    ),
    mirror(
        "largeInteger:lessThan:ifFail:",
        "<",
        Primitive::LargeInteger(LargeIntegerPrimitive::Less),
    ),
    mirror(
        "largeInteger:greaterThan:ifFail:",
        ">",
        Primitive::LargeInteger(LargeIntegerPrimitive::Greater),
    ),
    mirror(
        "largeInteger:lessOrEqual:ifFail:",
        "<=",
        Primitive::LargeInteger(LargeIntegerPrimitive::LessOrEqual),
    ),
    mirror(
        "largeInteger:greaterOrEqual:ifFail:",
        ">=",
        Primitive::LargeInteger(LargeIntegerPrimitive::GreaterOrEqual),
    ),
    mirror(
        "largeInteger:equals:ifFail:",
        "=",
        Primitive::LargeInteger(LargeIntegerPrimitive::Equal),
    ),
    mirror(
        "largeIntegerHash:ifFail:",
        "hash",
        Primitive::LargeInteger(LargeIntegerPrimitive::Hash),
    ),
    mirror(
        "largeIntegerNegated:ifFail:",
        "negated",
        Primitive::LargeInteger(LargeIntegerPrimitive::Negated),
    ),
    mirror(
        "largeIntegerAbsolute:ifFail:",
        "abs",
        Primitive::LargeInteger(LargeIntegerPrimitive::Absolute),
    ),
    mirror(
        "largeIntegerPrintString:ifFail:",
        "printString",
        Primitive::LargeInteger(LargeIntegerPrimitive::PrintString),
    ),
    mirror(
        "largeIntegerAsString:ifFail:",
        "asString",
        Primitive::LargeInteger(LargeIntegerPrimitive::PrintString),
    ),
    mirror(
        "float:add:ifFail:",
        "+",
        Primitive::Float(FloatPrimitive::Add),
    ),
    mirror(
        "float:subtract:ifFail:",
        "-",
        Primitive::Float(FloatPrimitive::Subtract),
    ),
    mirror(
        "float:multiply:ifFail:",
        "*",
        Primitive::Float(FloatPrimitive::Multiply),
    ),
    mirror(
        "float:divide:ifFail:",
        "/",
        Primitive::Float(FloatPrimitive::Divide),
    ),
    mirror(
        "float:lessThan:ifFail:",
        "<",
        Primitive::Float(FloatPrimitive::Less),
    ),
    mirror(
        "float:greaterThan:ifFail:",
        ">",
        Primitive::Float(FloatPrimitive::Greater),
    ),
    mirror(
        "float:equals:ifFail:",
        "=",
        Primitive::Float(FloatPrimitive::Equal),
    ),
    mirror(
        "floatHash:ifFail:",
        "hash",
        Primitive::Float(FloatPrimitive::Hash),
    ),
    mirror(
        "floatPrintString:ifFail:",
        "printString",
        Primitive::Float(FloatPrimitive::PrintString),
    ),
    mirror(
        "floatAsString:ifFail:",
        "asString",
        Primitive::Float(FloatPrimitive::PrintString),
    ),
    mirror(
        "stringSize:ifFail:",
        "size",
        Primitive::String(StringPrimitive::Size),
    ),
    mirror(
        "string:concatenate:ifFail:",
        ",",
        Primitive::String(StringPrimitive::Concatenate),
    ),
    mirror(
        "string:equals:ifFail:",
        "=",
        Primitive::String(StringPrimitive::Equal),
    ),
    mirror(
        "stringHash:ifFail:",
        "hash",
        Primitive::String(StringPrimitive::Hash),
    ),
    mirror(
        "stringAsSymbol:ifFail:",
        "asSymbol",
        Primitive::String(StringPrimitive::AsSymbol),
    ),
    mirror(
        "stringPrintString:ifFail:",
        "printString",
        Primitive::String(StringPrimitive::PrintString),
    ),
    mirror(
        "symbolAsString:ifFail:",
        "asString",
        Primitive::Symbol(SymbolPrimitive::AsString),
    ),
    mirror(
        "symbol:equals:ifFail:",
        "=",
        Primitive::Symbol(SymbolPrimitive::Equal),
    ),
    mirror(
        "symbolHash:ifFail:",
        "hash",
        Primitive::Symbol(SymbolPrimitive::Hash),
    ),
    mirror(
        "symbolPrintString:ifFail:",
        "printString",
        Primitive::Symbol(SymbolPrimitive::PrintString),
    ),
    mirror(
        "collection:do:ifFail:",
        "do:",
        Primitive::Collection(CollectionPrimitive::Enumerate),
    ),
    mirror(
        "collectionSize:ifFail:",
        "size",
        Primitive::Collection(CollectionPrimitive::Size),
    ),
    mirror(
        "collection:at:ifFail:",
        "at:",
        Primitive::Collection(CollectionPrimitive::ElementAt),
    ),
    mirror(
        "collection:at:put:ifFail:",
        "at:put:",
        Primitive::Collection(CollectionPrimitive::ElementAtPut),
    ),
    mirror(
        "collection:copyFrom:to:ifFail:",
        "copyFrom:to:",
        Primitive::Collection(CollectionPrimitive::CopyRange),
    ),
    mirror(
        "collection:replaceFrom:to:with:startingAt:ifFail:",
        "replaceFrom:to:with:startingAt:",
        Primitive::Collection(CollectionPrimitive::Replace),
    ),
    mirror(
        "blockValue:ifFail:",
        "value",
        Primitive::Block(BlockPrimitive::Value),
    ),
    mirror(
        "block:value:ifFail:",
        "value:",
        Primitive::Block(BlockPrimitive::Value),
    ),
    mirror(
        "block:value:value:ifFail:",
        "value:value:",
        Primitive::Block(BlockPrimitive::Value),
    ),
    mirror(
        "block:value:value:value:ifFail:",
        "value:value:value:",
        Primitive::Block(BlockPrimitive::Value),
    ),
    mirror(
        "blockNumArgs:ifFail:",
        "numArgs",
        Primitive::Block(BlockPrimitive::ArgumentCount),
    ),
    mirror(
        "block:whileTrue:ifFail:",
        "whileTrue:",
        Primitive::Block(BlockPrimitive::WhileTrue),
    ),
    mirror(
        "block:whileFalse:ifFail:",
        "whileFalse:",
        Primitive::Block(BlockPrimitive::WhileFalse),
    ),
    mirror(
        "transcript:show:ifFail:",
        "show:",
        Primitive::Platform(PlatformPrimitive::Show),
    ),
    mirror(
        "transcriptNewLine:ifFail:",
        "newLine",
        Primitive::Platform(PlatformPrimitive::NewLine),
    ),
    // A row's index never reaches an artifact -- `SendMirror` has no wire tag
    // and a decoded method carries no fast path -- so a row may be removed;
    // new rows are appended all the same, so a diff shows what was added.
    mirror(
        "className:ifFail:",
        "name",
        Primitive::Behavior(BehaviorPrimitive::Name),
    ),
    mirror(
        "metaclassThisClass:ifFail:",
        "thisClass",
        Primitive::Behavior(BehaviorPrimitive::ThisClass),
    ),
    mirror(
        "behavior:sharesApplicationWith:ifFail:",
        "sharesApplicationWith:",
        Primitive::Behavior(BehaviorPrimitive::SharesApplication),
    ),
    mirror(
        "behaviorEnclosing:ifFail:",
        "enclosingObject",
        Primitive::Behavior(BehaviorPrimitive::Enclosing),
    ),
    mirror(
        "behaviorSuperclass:ifFail:",
        "superclass",
        Primitive::Behavior(BehaviorPrimitive::Superclass),
    ),
    mirror(
        "currentActivation:ifFail:",
        "currentActivation",
        Primitive::Activation(ActivationPrimitive::Current),
    ),
    mirror(
        "activationSender:ifFail:",
        "sender",
        Primitive::Activation(ActivationPrimitive::Sender),
    ),
    mirror(
        "activationMethodName:ifFail:",
        "methodName",
        Primitive::Activation(ActivationPrimitive::MethodName),
    ),
    mirror(
        "activationIsActiveHandler:ifFail:",
        "isActiveHandler",
        Primitive::Activation(ActivationPrimitive::IsActiveHandler),
    ),
    mirror(
        "activationHandlerClass:ifFail:",
        "handlerClass",
        Primitive::Activation(ActivationPrimitive::HandlerClass),
    ),
    mirror(
        "activationHandlerBlock:ifFail:",
        "handlerBlock",
        Primitive::Activation(ActivationPrimitive::HandlerBlock),
    ),
    mirror(
        "activation:handlerActive:ifFail:",
        "handlerActive:",
        Primitive::Activation(ActivationPrimitive::SetHandlerActive),
    ),
    mirror(
        "activation:returnValue:ifFail:",
        "returnValue:",
        Primitive::Activation(ActivationPrimitive::ReturnValue),
    ),
    mirror(
        "activationRestart:ifFail:",
        "restart",
        Primitive::Activation(ActivationPrimitive::Restart),
    ),
    mirror(
        "object:inheritsMixinOf:ifFail:",
        "inheritsMixinOf:",
        Primitive::Activation(ActivationPrimitive::InheritsMixinOf),
    ),
    mirror(
        "terminate:selector:receiver:description:ifFail:",
        // Named with its three arguments, since a row's arity is counted
        // from its reported selector; it raises no error text of its own.
        "terminateSelector:receiver:description:",
        Primitive::Activation(ActivationPrimitive::Terminate),
    ),
    mirror(
        "message:sendTo:ifFail:",
        "sendTo:",
        Primitive::Message(MessagePrimitive::SendTo),
    ),
    mirror(
        "arrayClass:new:ifFail:",
        "new:",
        Primitive::Collection(CollectionPrimitive::Allocate),
    ),
    mirror(
        "moduleInstanceNamed:ifFail:",
        // Unary: the module's name is the row's subject.
        "moduleInstanceNamed",
        Primitive::Module(ModulePrimitive::InstanceNamed),
    ),
    mirror(
        "shippedModuleClassNamed:ifFail:",
        // Unary: the module's name is the row's subject.
        "shippedModuleClassNamed",
        Primitive::Module(ModulePrimitive::ClassNamed),
    ),
    mirror(
        "recordShippedModule:ifFail:",
        // Unary: the instance the module's factory answered is the row's
        // subject.
        "recordShippedModule",
        Primitive::Module(ModulePrimitive::Record),
    ),
    mirror(
        "futureValue:ifFail:",
        // Unary: the future is the row's subject.
        "value",
        Primitive::Future(FuturePrimitive::Value),
    ),
    mirror(
        "float:lessOrEqual:ifFail:",
        "<=",
        Primitive::Float(FloatPrimitive::LessOrEqual),
    ),
    mirror(
        "float:greaterOrEqual:ifFail:",
        ">=",
        Primitive::Float(FloatPrimitive::GreaterOrEqual),
    ),
    mirror(
        "integer:modulo:ifFail:",
        "\\\\",
        Primitive::Integer(IntegerPrimitive::Modulo),
    ),
    mirror(
        "largeInteger:modulo:ifFail:",
        "\\\\",
        Primitive::LargeInteger(LargeIntegerPrimitive::Modulo),
    ),
    mirror(
        "integer:bitAnd:ifFail:",
        "bitAnd:",
        Primitive::Integer(IntegerPrimitive::BitAnd),
    ),
    mirror(
        "largeInteger:bitAnd:ifFail:",
        "bitAnd:",
        Primitive::LargeInteger(LargeIntegerPrimitive::BitAnd),
    ),
    mirror(
        "integerAsFloat:ifFail:",
        "asFloat",
        Primitive::Integer(IntegerPrimitive::AsFloat),
    ),
    mirror(
        "largeIntegerAsFloat:ifFail:",
        "asFloat",
        Primitive::LargeInteger(LargeIntegerPrimitive::AsFloat),
    ),
    mirror(
        "float:asStringFixed:ifFail:",
        "asStringFixed:",
        Primitive::Float(FloatPrimitive::AsStringFixed),
    ),
    mirror(
        "float:asStringExponential:ifFail:",
        "asStringExponential:",
        Primitive::Float(FloatPrimitive::AsStringExponential),
    ),
    mirror(
        "float:asStringPrecision:ifFail:",
        "asStringPrecision:",
        Primitive::Float(FloatPrimitive::AsStringPrecision),
    ),
    mirror(
        "floatClass:parse:ifFail:",
        "parse:",
        Primitive::Float(FloatPrimitive::Parse),
    ),
    mirror(
        "clockMonotonicNanoseconds:ifFail:",
        "currentMonotonicNanoseconds",
        Primitive::Platform(PlatformPrimitive::MonotonicNanoseconds),
    ),
    mirror(
        "integer:bitOr:ifFail:",
        "bitOr:",
        Primitive::Integer(IntegerPrimitive::BitOr),
    ),
    mirror(
        "largeInteger:bitOr:ifFail:",
        "bitOr:",
        Primitive::LargeInteger(LargeIntegerPrimitive::BitOr),
    ),
    mirror(
        "integer:bitXor:ifFail:",
        "bitXor:",
        Primitive::Integer(IntegerPrimitive::BitXor),
    ),
    mirror(
        "largeInteger:bitXor:ifFail:",
        "bitXor:",
        Primitive::LargeInteger(LargeIntegerPrimitive::BitXor),
    ),
    mirror(
        "integer:shiftLeft:ifFail:",
        "<<",
        Primitive::Integer(IntegerPrimitive::ShiftLeft),
    ),
    mirror(
        "largeInteger:shiftLeft:ifFail:",
        "<<",
        Primitive::LargeInteger(LargeIntegerPrimitive::ShiftLeft),
    ),
    mirror(
        "integer:gcd:ifFail:",
        "gcd:",
        Primitive::AnyInteger(AnyIntegerPrimitive::Gcd),
    ),
    mirror(
        "integer:shiftRight:ifFail:",
        ">>",
        Primitive::AnyInteger(AnyIntegerPrimitive::ShiftRight),
    ),
    mirror(
        "integer:digitAt:ifFail:",
        "digitAt:",
        Primitive::AnyInteger(AnyIntegerPrimitive::DigitAt),
    ),
    mirror(
        "integerDigitLength:ifFail:",
        "digitLength",
        Primitive::AnyInteger(AnyIntegerPrimitive::DigitLength),
    ),
    mirror(
        "integer:asStringRadix:ifFail:",
        "asStringRadix:",
        Primitive::AnyInteger(AnyIntegerPrimitive::AsStringRadix),
    ),
    mirror(
        "integerClass:parse:radix:ifFail:",
        "parse:radix:",
        Primitive::AnyInteger(AnyIntegerPrimitive::Parse),
    ),
    mirror(
        "floatSquareRoot:ifFail:",
        "sqrt",
        Primitive::Float(FloatPrimitive::SquareRoot),
    ),
    mirror(
        "string:at:ifFail:",
        "at:",
        Primitive::String(StringPrimitive::ElementAt),
    ),
    mirror(
        "string:copyFrom:to:ifFail:",
        "copyFrom:to:",
        Primitive::String(StringPrimitive::CopyRange),
    ),
    mirror(
        "string:indexOf:startingAt:ifFail:",
        "indexOf:startingAt:",
        Primitive::String(StringPrimitive::IndexOf),
    ),
    mirror(
        "string:lastIndexOf:startingAt:ifFail:",
        "lastIndexOf:startingAt:",
        Primitive::String(StringPrimitive::LastIndexOf),
    ),
    mirror(
        "stringClass:withAll:ifFail:",
        "withAll:",
        Primitive::String(StringPrimitive::FromCodePoints),
    ),
    mirror(
        "stringClass:with:ifFail:",
        "with:",
        Primitive::String(StringPrimitive::FromCodePoint),
    ),
    mirror(
        "floatSine:ifFail:",
        "sin",
        Primitive::Float(FloatPrimitive::Sine),
    ),
    mirror(
        "floatCosine:ifFail:",
        "cos",
        Primitive::Float(FloatPrimitive::Cosine),
    ),
    mirror(
        "floatTangent:ifFail:",
        "tan",
        Primitive::Float(FloatPrimitive::Tangent),
    ),
    mirror(
        "floatArcSine:ifFail:",
        "asin",
        Primitive::Float(FloatPrimitive::ArcSine),
    ),
    mirror(
        "floatArcCosine:ifFail:",
        "acos",
        Primitive::Float(FloatPrimitive::ArcCosine),
    ),
    mirror(
        "floatArcTangent:ifFail:",
        "atan",
        Primitive::Float(FloatPrimitive::ArcTangent),
    ),
    mirror(
        "float:arcTangent:ifFail:",
        "atan:",
        Primitive::Float(FloatPrimitive::ArcTangentOf),
    ),
    mirror(
        "floatExponential:ifFail:",
        "exp",
        Primitive::Float(FloatPrimitive::Exponential),
    ),
    mirror(
        "floatNaturalLogarithm:ifFail:",
        "ln",
        Primitive::Float(FloatPrimitive::NaturalLogarithm),
    ),
    mirror(
        "floatLogarithm:ifFail:",
        "log",
        Primitive::Float(FloatPrimitive::Logarithm),
    ),
    mirror(
        "float:raisedTo:ifFail:",
        "raisedTo:",
        Primitive::Float(FloatPrimitive::RaisedTo),
    ),
    mirror(
        "floatFloor:ifFail:",
        "floor",
        Primitive::Float(FloatPrimitive::Floor),
    ),
    mirror(
        "floatCeiling:ifFail:",
        "ceiling",
        Primitive::Float(FloatPrimitive::Ceiling),
    ),
    mirror(
        "floatRounded:ifFail:",
        "rounded",
        Primitive::Float(FloatPrimitive::Rounded),
    ),
    mirror(
        "floatTruncated:ifFail:",
        "truncated",
        Primitive::Float(FloatPrimitive::Truncated),
    ),
    mirror(
        "floatNegated:ifFail:",
        "negated",
        Primitive::Float(FloatPrimitive::Negated),
    ),
    mirror(
        "floatAbsolute:ifFail:",
        "abs",
        Primitive::Float(FloatPrimitive::Absolute),
    ),
    mirror(
        "floatIsNaN:ifFail:",
        "isNaN",
        Primitive::Float(FloatPrimitive::IsNaN),
    ),
    mirror(
        "floatIsFinite:ifFail:",
        "isFinite",
        Primitive::Float(FloatPrimitive::IsFinite),
    ),
    mirror(
        "floatIsInfinite:ifFail:",
        "isInfinite",
        Primitive::Float(FloatPrimitive::IsInfinite),
    ),
    mirror(
        "floatAsInteger:ifFail:",
        "asInteger",
        Primitive::Float(FloatPrimitive::AsInteger),
    ),
    mirror(
        "fractionNumerator:ifFail:",
        "numerator",
        Primitive::Fraction(FractionPrimitive::Numerator),
    ),
    mirror(
        "fractionDenominator:ifFail:",
        "denominator",
        Primitive::Fraction(FractionPrimitive::Denominator),
    ),
    mirror(
        "fractionClass:numerator:denominator:ifFail:",
        "numerator:denominator:",
        Primitive::Fraction(FractionPrimitive::Make),
    ),
    mirror(
        "fractionClass:reducedNumerator:denominator:ifFail:",
        "reducedNumerator:denominator:",
        Primitive::Fraction(FractionPrimitive::MakeReduced),
    ),
    mirror(
        "number:lessThan:ifFail:",
        "<",
        Primitive::Number(NumberPrimitive::Less),
    ),
    mirror(
        "number:greaterThan:ifFail:",
        ">",
        Primitive::Number(NumberPrimitive::Greater),
    ),
    mirror(
        "number:lessOrEqual:ifFail:",
        "<=",
        Primitive::Number(NumberPrimitive::LessOrEqual),
    ),
    mirror(
        "number:greaterOrEqual:ifFail:",
        ">=",
        Primitive::Number(NumberPrimitive::GreaterOrEqual),
    ),
    mirror(
        "number:equals:ifFail:",
        "=",
        Primitive::Number(NumberPrimitive::Equal),
    ),
    mirror(
        "numberHash:ifFail:",
        "hash",
        Primitive::Number(NumberPrimitive::Hash),
    ),
    mirror(
        "number:add:ifFail:",
        "+",
        Primitive::Number(NumberPrimitive::Add),
    ),
    mirror(
        "number:subtract:ifFail:",
        "-",
        Primitive::Number(NumberPrimitive::Subtract),
    ),
    mirror(
        "number:multiply:ifFail:",
        "*",
        Primitive::Number(NumberPrimitive::Multiply),
    ),
    mirror(
        "number:divide:ifFail:",
        "/",
        Primitive::Number(NumberPrimitive::Divide),
    ),
    mirror(
        "number:floorDivide:ifFail:",
        "//",
        Primitive::Number(NumberPrimitive::FloorDivide),
    ),
    mirror(
        "number:modulo:ifFail:",
        "\\\\",
        Primitive::Number(NumberPrimitive::Modulo),
    ),
    mirror(
        "number:quotient:ifFail:",
        "quo:",
        Primitive::Number(NumberPrimitive::Quotient),
    ),
    mirror(
        "number:remainder:ifFail:",
        "rem:",
        Primitive::Number(NumberPrimitive::Remainder),
    ),
    mirror(
        "numberAsFloat:ifFail:",
        "asFloat",
        Primitive::Number(NumberPrimitive::AsFloat),
    ),
    mirror(
        "futureMade:ifFail:",
        // Unary: the new future is the row's subject, as the future is
        // `futureValue:ifFail:`'s. It never fails, so the selector names
        // what the program sent.
        "computing",
        Primitive::Future(FuturePrimitive::Made),
    ),
    mirror(
        "byteArrayClass:new:ifFail:",
        "new:",
        Primitive::Bytes(BytePrimitive::Allocate),
    ),
    mirror(
        "byteArrayClass:fits:ifFail:",
        // It never fails -- it answers a Boolean or declines -- so the
        // selector names the message whose allocation it answers for.
        "new:",
        Primitive::Bytes(BytePrimitive::Fits),
    ),
    mirror(
        "byteArraySize:ifFail:",
        "size",
        Primitive::Bytes(BytePrimitive::Size),
    ),
    mirror(
        "byteArray:at:ifFail:",
        "at:",
        Primitive::Bytes(BytePrimitive::ElementAt),
    ),
    mirror(
        "byteArray:at:put:ifFail:",
        "at:put:",
        Primitive::Bytes(BytePrimitive::ElementAtPut),
    ),
    mirror(
        "byteArray:copyFrom:to:ifFail:",
        "copyFrom:to:",
        Primitive::Bytes(BytePrimitive::CopyRange),
    ),
    mirror(
        "byteArray:replaceFrom:to:with:startingAt:ifFail:",
        "replaceFrom:to:with:startingAt:",
        Primitive::Bytes(BytePrimitive::Replace),
    ),
    mirror(
        "byteArray:concatenate:ifFail:",
        ",",
        Primitive::Bytes(BytePrimitive::Concatenate),
    ),
    mirror(
        "byteArray:indexOf:startingAt:ifFail:",
        "indexOf:startingAt:",
        Primitive::Bytes(BytePrimitive::IndexOf),
    ),
    mirror(
        "byteArray:lastIndexOf:startingAt:ifFail:",
        "lastIndexOf:startingAt:",
        Primitive::Bytes(BytePrimitive::LastIndexOf),
    ),
    mirror(
        "byteArray:startsWith:ifFail:",
        "startsWith:",
        Primitive::Bytes(BytePrimitive::StartsWith),
    ),
    mirror(
        "byteArray:endsWith:ifFail:",
        "endsWith:",
        Primitive::Bytes(BytePrimitive::EndsWith),
    ),
    mirror(
        "stringAsUtf8:ifFail:",
        "asUtf8",
        Primitive::String(StringPrimitive::AsUtf8),
    ),
    mirror(
        "stringClass:fromUtf8:from:to:ifFail:",
        "fromUtf8:from:to:",
        Primitive::String(StringPrimitive::FromUtf8),
    ),
    mirror(
        "stringUtf8Size:ifFail:",
        "utf8Size",
        Primitive::String(StringPrimitive::Utf8Size),
    ),
    mirror(
        "byteArray:at:putCodePoint:ifFail:",
        "at:putCodePoint:",
        Primitive::Bytes(BytePrimitive::WriteCodePoint),
    ),
    // The typed arrays' elements, little-endian at a 1-based element
    // index; `Uint8Array` uses `ByteArray`'s own `at:` and `at:put:`.
    mirror(
        "byteArray:int8ElementAt:ifFail:",
        "at:",
        Primitive::Element(ElementAccess::new(ElementKind::Int8, false, false, false)),
    ),
    mirror(
        "byteArray:int8ElementAt:put:ifFail:",
        "at:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Int8, false, false, true)),
    ),
    mirror(
        "byteArray:int16ElementAt:ifFail:",
        "at:",
        Primitive::Element(ElementAccess::new(ElementKind::Int16, false, false, false)),
    ),
    mirror(
        "byteArray:int16ElementAt:put:ifFail:",
        "at:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Int16, false, false, true)),
    ),
    mirror(
        "byteArray:uint16ElementAt:ifFail:",
        "at:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint16, false, false, false)),
    ),
    mirror(
        "byteArray:uint16ElementAt:put:ifFail:",
        "at:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint16, false, false, true)),
    ),
    mirror(
        "byteArray:int32ElementAt:ifFail:",
        "at:",
        Primitive::Element(ElementAccess::new(ElementKind::Int32, false, false, false)),
    ),
    mirror(
        "byteArray:int32ElementAt:put:ifFail:",
        "at:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Int32, false, false, true)),
    ),
    mirror(
        "byteArray:uint32ElementAt:ifFail:",
        "at:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint32, false, false, false)),
    ),
    mirror(
        "byteArray:uint32ElementAt:put:ifFail:",
        "at:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint32, false, false, true)),
    ),
    mirror(
        "byteArray:int64ElementAt:ifFail:",
        "at:",
        Primitive::Element(ElementAccess::new(ElementKind::Int64, false, false, false)),
    ),
    mirror(
        "byteArray:int64ElementAt:put:ifFail:",
        "at:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Int64, false, false, true)),
    ),
    mirror(
        "byteArray:uint64ElementAt:ifFail:",
        "at:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint64, false, false, false)),
    ),
    mirror(
        "byteArray:uint64ElementAt:put:ifFail:",
        "at:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint64, false, false, true)),
    ),
    mirror(
        "byteArray:float32ElementAt:ifFail:",
        "at:",
        Primitive::Element(ElementAccess::new(
            ElementKind::Float32,
            false,
            false,
            false,
        )),
    ),
    mirror(
        "byteArray:float32ElementAt:put:ifFail:",
        "at:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Float32, false, false, true)),
    ),
    mirror(
        "byteArray:float64ElementAt:ifFail:",
        "at:",
        Primitive::Element(ElementAccess::new(
            ElementKind::Float64,
            false,
            false,
            false,
        )),
    ),
    mirror(
        "byteArray:float64ElementAt:put:ifFail:",
        "at:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Float64, false, false, true)),
    ),
    // `DataView`'s accessors at a 1-based byte position: a single byte
    // has no order, every wider kind a row for each.
    mirror(
        "byteArray:int8At:ifFail:",
        "int8At:",
        Primitive::Element(ElementAccess::new(ElementKind::Int8, false, true, false)),
    ),
    mirror(
        "byteArray:int8At:put:ifFail:",
        "int8At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Int8, false, true, true)),
    ),
    mirror(
        "byteArray:uint8At:ifFail:",
        "uint8At:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint8, false, true, false)),
    ),
    mirror(
        "byteArray:uint8At:put:ifFail:",
        "uint8At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint8, false, true, true)),
    ),
    mirror(
        "byteArray:littleEndianInt16At:ifFail:",
        "int16At:",
        Primitive::Element(ElementAccess::new(ElementKind::Int16, false, true, false)),
    ),
    mirror(
        "byteArray:littleEndianInt16At:put:ifFail:",
        "int16At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Int16, false, true, true)),
    ),
    mirror(
        "byteArray:littleEndianUint16At:ifFail:",
        "uint16At:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint16, false, true, false)),
    ),
    mirror(
        "byteArray:littleEndianUint16At:put:ifFail:",
        "uint16At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint16, false, true, true)),
    ),
    mirror(
        "byteArray:littleEndianInt32At:ifFail:",
        "int32At:",
        Primitive::Element(ElementAccess::new(ElementKind::Int32, false, true, false)),
    ),
    mirror(
        "byteArray:littleEndianInt32At:put:ifFail:",
        "int32At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Int32, false, true, true)),
    ),
    mirror(
        "byteArray:littleEndianUint32At:ifFail:",
        "uint32At:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint32, false, true, false)),
    ),
    mirror(
        "byteArray:littleEndianUint32At:put:ifFail:",
        "uint32At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint32, false, true, true)),
    ),
    mirror(
        "byteArray:littleEndianInt64At:ifFail:",
        "int64At:",
        Primitive::Element(ElementAccess::new(ElementKind::Int64, false, true, false)),
    ),
    mirror(
        "byteArray:littleEndianInt64At:put:ifFail:",
        "int64At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Int64, false, true, true)),
    ),
    mirror(
        "byteArray:littleEndianUint64At:ifFail:",
        "uint64At:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint64, false, true, false)),
    ),
    mirror(
        "byteArray:littleEndianUint64At:put:ifFail:",
        "uint64At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint64, false, true, true)),
    ),
    mirror(
        "byteArray:littleEndianFloat32At:ifFail:",
        "float32At:",
        Primitive::Element(ElementAccess::new(ElementKind::Float32, false, true, false)),
    ),
    mirror(
        "byteArray:littleEndianFloat32At:put:ifFail:",
        "float32At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Float32, false, true, true)),
    ),
    mirror(
        "byteArray:littleEndianFloat64At:ifFail:",
        "float64At:",
        Primitive::Element(ElementAccess::new(ElementKind::Float64, false, true, false)),
    ),
    mirror(
        "byteArray:littleEndianFloat64At:put:ifFail:",
        "float64At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Float64, false, true, true)),
    ),
    mirror(
        "byteArray:bigEndianInt16At:ifFail:",
        "int16At:",
        Primitive::Element(ElementAccess::new(ElementKind::Int16, true, true, false)),
    ),
    mirror(
        "byteArray:bigEndianInt16At:put:ifFail:",
        "int16At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Int16, true, true, true)),
    ),
    mirror(
        "byteArray:bigEndianUint16At:ifFail:",
        "uint16At:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint16, true, true, false)),
    ),
    mirror(
        "byteArray:bigEndianUint16At:put:ifFail:",
        "uint16At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint16, true, true, true)),
    ),
    mirror(
        "byteArray:bigEndianInt32At:ifFail:",
        "int32At:",
        Primitive::Element(ElementAccess::new(ElementKind::Int32, true, true, false)),
    ),
    mirror(
        "byteArray:bigEndianInt32At:put:ifFail:",
        "int32At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Int32, true, true, true)),
    ),
    mirror(
        "byteArray:bigEndianUint32At:ifFail:",
        "uint32At:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint32, true, true, false)),
    ),
    mirror(
        "byteArray:bigEndianUint32At:put:ifFail:",
        "uint32At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint32, true, true, true)),
    ),
    mirror(
        "byteArray:bigEndianInt64At:ifFail:",
        "int64At:",
        Primitive::Element(ElementAccess::new(ElementKind::Int64, true, true, false)),
    ),
    mirror(
        "byteArray:bigEndianInt64At:put:ifFail:",
        "int64At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Int64, true, true, true)),
    ),
    mirror(
        "byteArray:bigEndianUint64At:ifFail:",
        "uint64At:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint64, true, true, false)),
    ),
    mirror(
        "byteArray:bigEndianUint64At:put:ifFail:",
        "uint64At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Uint64, true, true, true)),
    ),
    mirror(
        "byteArray:bigEndianFloat32At:ifFail:",
        "float32At:",
        Primitive::Element(ElementAccess::new(ElementKind::Float32, true, true, false)),
    ),
    mirror(
        "byteArray:bigEndianFloat32At:put:ifFail:",
        "float32At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Float32, true, true, true)),
    ),
    mirror(
        "byteArray:bigEndianFloat64At:ifFail:",
        "float64At:",
        Primitive::Element(ElementAccess::new(ElementKind::Float64, true, true, false)),
    ),
    mirror(
        "byteArray:bigEndianFloat64At:put:ifFail:",
        "float64At:put:",
        Primitive::Element(ElementAccess::new(ElementKind::Float64, true, true, true)),
    ),
    mirror(
        "weakArrayClass:new:ifFail:",
        "new:",
        Primitive::Weak(WeakPrimitive::Allocate),
    ),
    mirror(
        "weakArraySize:ifFail:",
        "size",
        Primitive::Weak(WeakPrimitive::Size),
    ),
    mirror(
        "weakArray:at:ifFail:",
        "at:",
        Primitive::Weak(WeakPrimitive::ElementAt),
    ),
    mirror(
        "weakArray:at:put:ifFail:",
        "at:put:",
        Primitive::Weak(WeakPrimitive::ElementAtPut),
    ),
    mirror(
        "garbageCollect:ifFail:",
        "garbageCollect",
        Primitive::Weak(WeakPrimitive::Collect),
    ),
    // Reported `isValue`, no kernel selector: a row's arity is its
    // reported selector's, this leaf takes its subject alone, and it
    // raises nothing whose text the selector would name.
    mirror(
        "objectIsValue:ifFail:",
        "isValue",
        Primitive::Weak(WeakPrimitive::IsValue),
    ),
    mirror(
        "ephemeronKey:ifFail:",
        "key",
        Primitive::Weak(WeakPrimitive::EphemeronKey),
    ),
    mirror(
        "ephemeron:key:ifFail:",
        "key:",
        Primitive::Weak(WeakPrimitive::EphemeronSetKey),
    ),
    mirror(
        "ephemeronValue:ifFail:",
        "value",
        Primitive::Weak(WeakPrimitive::EphemeronValue),
    ),
    mirror(
        "ephemeron:value:ifFail:",
        "value:",
        Primitive::Weak(WeakPrimitive::EphemeronSetValue),
    ),
    mirror(
        "ephemeronFinalizer:ifFail:",
        "finalizer",
        Primitive::Weak(WeakPrimitive::EphemeronFinalizer),
    ),
    mirror(
        "ephemeron:finalizer:ifFail:",
        "finalizer:",
        Primitive::Weak(WeakPrimitive::EphemeronSetFinalizer),
    ),
    mirror(
        "reflectionMixinOf:ifFail:",
        "mixin",
        Primitive::Reflection(ReflectionPrimitive::MixinOf),
    ),
    mirror(
        "reflectionIsMetaclass:ifFail:",
        "isMeta",
        Primitive::Reflection(ReflectionPrimitive::IsMetaclass),
    ),
    mirror(
        "reflectionEnclosingObjectOf:ifFail:",
        "enclosingObject",
        Primitive::Reflection(ReflectionPrimitive::EnclosingObjectOf),
    ),
    mirror(
        "reflectionSlotNamesOf:ifFail:",
        "slots",
        Primitive::Reflection(ReflectionPrimitive::SlotNamesOf),
    ),
    mirror(
        "reflectionSlotOf:named:ifFail:",
        "getSlot:",
        Primitive::Reflection(ReflectionPrimitive::SlotNamed),
    ),
    mirror(
        "reflectionMixinName:ifFail:",
        "name",
        Primitive::Reflection(ReflectionPrimitive::MixinName),
    ),
    mirror(
        "reflectionMixinIsMeta:ifFail:",
        "isMeta",
        Primitive::Reflection(ReflectionPrimitive::MixinIsMeta),
    ),
    mirror(
        "reflectionMixinEnclosing:ifFail:",
        "enclosingMixin",
        Primitive::Reflection(ReflectionPrimitive::MixinEnclosing),
    ),
    mirror(
        "reflectionMixinInstanceSide:ifFail:",
        "instanceMixin",
        Primitive::Reflection(ReflectionPrimitive::MixinInstanceSide),
    ),
    mirror(
        "reflectionMixinClassSide:ifFail:",
        "classMixin",
        Primitive::Reflection(ReflectionPrimitive::MixinClassSide),
    ),
    mirror(
        "reflectionMixinSlots:ifFail:",
        "slots",
        Primitive::Reflection(ReflectionPrimitive::MixinSlots),
    ),
    mirror(
        "reflectionMixinMethods:ifFail:",
        "methods",
        Primitive::Reflection(ReflectionPrimitive::MixinMethods),
    ),
    mirror(
        "reflectionMixinNested:ifFail:",
        "nestedClasses",
        Primitive::Reflection(ReflectionPrimitive::MixinNested),
    ),
    mirror(
        "reflectionMixinAccess:ifFail:",
        "accessModifier",
        Primitive::Reflection(ReflectionPrimitive::MixinAccess),
    ),
    mirror(
        "reflectionMixin:methodSource:ifFail:",
        "methodSource:",
        Primitive::Reflection(ReflectionPrimitive::MethodSource),
    ),
    mirror(
        "reflectionMixinSource:ifFail:",
        "source",
        Primitive::Reflection(ReflectionPrimitive::MixinSource),
    ),
    mirror(
        "reflectionMixinIsShipped:ifFail:",
        "isShipped",
        Primitive::Reflection(ReflectionPrimitive::MixinIsShipped),
    ),
    mirror(
        "reflectionObject:identicalTo:ifFail:",
        "==",
        Primitive::Reflection(ReflectionPrimitive::Identical),
    ),
    mirror(
        "reflectionSymbolText:ifFail:",
        "asString",
        Primitive::Reflection(ReflectionPrimitive::SymbolText),
    ),
    // Reported `forwardIdentityTo:`, no kernel selector: a row's arity is
    // its reported selector's, this leaf takes the subject and one
    // argument, and it raises nothing whose text the selector would name.
    mirror(
        "elementsOf:forwardIdentityToElementsOf:ifFail:",
        "forwardIdentityTo:",
        Primitive::Reflection(ReflectionPrimitive::ForwardIdentity),
    ),
];

const _: () = assert!(MIRROR.len() <= u16::MAX as usize + 1);

/// The row `primitive` names.
pub fn mirror_entry(primitive: MirrorPrimitive) -> &'static MirrorEntry {
    &MIRROR[primitive.index()]
}

/// The row whose mirror selector is `selector`, for the compiler, which
/// binds a recognized mirror send to its row once, at compile time.
pub fn mirror_primitive_named(selector: &str) -> Option<MirrorPrimitive> {
    MIRROR
        .iter()
        .position(|entry| entry.selector == selector)
        .map(|index| MirrorPrimitive(index as u16))
}

/// The VM mirror's method table: one public `MethodEntry::Primitive` per
/// row of `MIRROR`. It is the only method table the bootstrap builds in
/// Rust -- every other builtin's methods are `Kernel.ns`'s.
pub fn mirror_method_table() -> IdentityMap<Symbol, MethodEntry> {
    MIRROR
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            (
                Symbol::intern(entry.selector),
                MethodEntry::Primitive(PrimitiveMethod {
                    primitive: Primitive::Mirror(MirrorPrimitive(index as u16)),
                    access: ast::AccessModifier::Public,
                }),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row index past 255 is representable, so the table can grow past
    /// 256 rows.
    #[test]
    fn the_mirror_table_may_pass_256_rows() {
        let row: MirrorPrimitive = MirrorPrimitive(300);
        assert_eq!(row.index(), 300);
    }

    /// `Integer`'s `+` is a `Kernel.ns` method: the leaf it forwards
    /// to is the mirror row named `integer:add:ifFail:`, still the same
    /// `IntegerPrimitive::Add` the family's module executes.
    #[test]
    fn integer_addition_is_its_own_mirror_primitive() {
        let primitive = mirror_primitive_named("integer:add:ifFail:").expect("row exists");
        let entry = mirror_entry(primitive);
        assert_eq!(entry.primitive, Primitive::Integer(IntegerPrimitive::Add));
        assert_eq!(entry.reported, "+");
    }
}
