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
    Float(FloatPrimitive),
    String(StringPrimitive),
    Symbol(SymbolPrimitive),
    Collection(CollectionPrimitive),
    Block(BlockPrimitive),
    Platform(PlatformPrimitive),
    Activation(ActivationPrimitive),
    Message(MessagePrimitive),
    /// `moduleNamed:`: a shipped module's root class (`builtins::module`).
    Module(ModulePrimitive),
    /// §6.3.2's pull future, forced by its `doesNotUnderstand:`
    /// (`builtins::future`).
    Future(FuturePrimitive),
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
    /// forwards or ignores the operand (`ElementAtPut`'s element, `Block`'s
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
                | IntegerPrimitive::Less
                | IntegerPrimitive::Greater
                | IntegerPrimitive::LessOrEqual
                | IntegerPrimitive::GreaterOrEqual
                | IntegerPrimitive::Negated
                | IntegerPrimitive::Absolute
                | IntegerPrimitive::PrintString,
            ) => false,
            Primitive::LargeInteger(LargeIntegerPrimitive::Equal) => true,
            Primitive::LargeInteger(
                LargeIntegerPrimitive::Add
                | LargeIntegerPrimitive::Subtract
                | LargeIntegerPrimitive::Multiply
                | LargeIntegerPrimitive::FloorDivide
                | LargeIntegerPrimitive::Less
                | LargeIntegerPrimitive::Greater
                | LargeIntegerPrimitive::LessOrEqual
                | LargeIntegerPrimitive::GreaterOrEqual
                | LargeIntegerPrimitive::Hash
                | LargeIntegerPrimitive::Negated
                | LargeIntegerPrimitive::Absolute
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
                | FloatPrimitive::Hash
                | FloatPrimitive::PrintString,
            ) => false,
            Primitive::String(StringPrimitive::Equal) => true,
            Primitive::String(
                StringPrimitive::Size
                | StringPrimitive::Concatenate
                | StringPrimitive::Hash
                | StringPrimitive::AsSymbol
                | StringPrimitive::PrintString,
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
                | CollectionPrimitive::Allocate,
            ) => false,
            Primitive::Block(
                BlockPrimitive::Value
                | BlockPrimitive::ArgumentCount
                | BlockPrimitive::WhileTrue
                | BlockPrimitive::WhileFalse,
            ) => false,
            Primitive::Platform(PlatformPrimitive::Show | PlatformPrimitive::NewLine) => false,
            Primitive::Activation(ActivationPrimitive::InheritsMixinOf) => true,
            Primitive::Activation(
                ActivationPrimitive::Current
                | ActivationPrimitive::Sender
                | ActivationPrimitive::IsActiveHandler
                | ActivationPrimitive::HandlerClass
                | ActivationPrimitive::HandlerBlock
                | ActivationPrimitive::SetHandlerActive
                | ActivationPrimitive::ReturnValue
                | ActivationPrimitive::Restart
                | ActivationPrimitive::Terminate,
            ) => false,
            Primitive::Message(MessagePrimitive::SendTo) => false,
            Primitive::Module(ModulePrimitive::Named) => false,
            Primitive::Future(FuturePrimitive::Value) => false,
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
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
    Equal,
    Negated,
    Absolute,
    PrintString,
}

/// `LargeInteger`'s leaves: `Integer`'s protocol for a receiver whose
/// payload is `ObjectData::LargeInteger`. A family of its own rather than
/// `Integer`'s leaves widened, so `IntegerPrimitive` keeps meaning "the
/// receiver is a `SmallInteger`", which is what a JIT tier emits inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LargeIntegerPrimitive {
    Add,
    Subtract,
    Multiply,
    FloorDivide,
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
    Equal,
    Hash,
    Negated,
    Absolute,
    PrintString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatPrimitive {
    Add,
    Subtract,
    Multiply,
    Divide,
    Less,
    Greater,
    Equal,
    Hash,
    PrintString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringPrimitive {
    Size,
    Concatenate,
    Equal,
    Hash,
    AsSymbol,
    PrintString,
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
    IsActiveHandler,
    HandlerClass,
    HandlerBlock,
    SetHandlerActive,
    ReturnValue,
    Restart,
    InheritsMixinOf,
    Terminate,
}

/// `Message>>sentTo:`'s leaf: the message sent to a receiver as an
/// ordinary public send (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePrimitive {
    SendTo,
}

/// The module table's one leaf (`docs/design/core-library.md`, "The module
/// table and the boot").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModulePrimitive {
    Named,
}

/// The one leaf of `Kernel.ns`'s `Future` (`docs/design/object-model.md`,
/// "Simultaneous slots").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FuturePrimitive {
    /// The future's value, forcing it first when it is unresolved.
    Value,
}

/// A row of `MIRROR`. A `u8`, because `Primitive` must stay two bytes: a
/// `Resolved::Primitive` carries one, and its size is pinned
/// (`interpreter::resolve`'s
/// `a_resolution_stays_the_size_the_hot_path_was_measured_at`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorPrimitive(u8);

impl MirrorPrimitive {
    pub fn index(self) -> usize {
        self.0 as usize
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
        "sentTo:",
        Primitive::Message(MessagePrimitive::SendTo),
    ),
    mirror(
        "arrayClass:new:ifFail:",
        "new:",
        Primitive::Collection(CollectionPrimitive::Allocate),
    ),
    mirror(
        "moduleNamed:ifFail:",
        // Unary: the module's name is the row's subject.
        "moduleNamed",
        Primitive::Module(ModulePrimitive::Named),
    ),
    mirror(
        "futureValue:ifFail:",
        // Unary: the future is the row's subject.
        "value",
        Primitive::Future(FuturePrimitive::Value),
    ),
];

const _: () = assert!(MIRROR.len() <= u8::MAX as usize + 1);

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
        .map(|index| MirrorPrimitive(index as u8))
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
                    primitive: Primitive::Mirror(MirrorPrimitive(index as u8)),
                    access: ast::AccessModifier::Public,
                }),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
