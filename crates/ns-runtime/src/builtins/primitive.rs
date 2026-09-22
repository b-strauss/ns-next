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
    Boolean(BooleanPrimitive),
    Integer(IntegerPrimitive),
    Float(FloatPrimitive),
    String(StringPrimitive),
    Symbol(SymbolPrimitive),
    Character(CharacterPrimitive),
    Collection(CollectionPrimitive),
    Block(BlockPrimitive),
    Platform(PlatformPrimitive),
    /// A leaf reached as a send to the VM mirror: a row of `MIRROR`.
    Mirror(MirrorPrimitive),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectPrimitive {
    Class,
    Identical,
    Equal,
    Hash,
    PrintString,
    DoesNotUnderstand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanPrimitive {
    IfTrueIfFalse,
    IfFalseIfTrue,
    IfTrue,
    IfFalse,
    Not,
    And,
    Or,
    PrintString,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatPrimitive {
    Add,
    Subtract,
    Multiply,
    Divide,
    Less,
    Greater,
    Equal,
    PrintString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringPrimitive {
    Size,
    Concatenate,
    Equal,
    AsSymbol,
    PrintString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolPrimitive {
    AsString,
    Equal,
    PrintString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterPrimitive {
    AsInteger,
    AsString,
    Equal,
}

/// `Array` and `OrderedCollection` share their payload, so they share their
/// primitives; each class's list names only the ones it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionPrimitive {
    Enumerate,
    Size,
    ElementAt,
    ElementAtPut,
    Add,
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
    Transcript,
    ObjectClass,
    ArrayClass,
    OrderedCollectionClass,
    StringClass,
    SymbolClass,
    Collections,
    List,
    Show,
    NewLine,
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
/// position here, which `MirrorPrimitive` carries. `Array` and `OrderedCollection` share
/// rows 49-53, as they share their payload; each of their kernel classes
/// forwards only the selectors `Kernel.ns` declares for it.
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
        "object:doesNotUnderstand:ifFail:",
        "doesNotUnderstand:",
        Primitive::Object(ObjectPrimitive::DoesNotUnderstand),
    ),
    mirror(
        "boolean:ifTrue:ifFalse:ifFail:",
        "ifTrue:ifFalse:",
        Primitive::Boolean(BooleanPrimitive::IfTrueIfFalse),
    ),
    mirror(
        "boolean:ifFalse:ifTrue:ifFail:",
        "ifFalse:ifTrue:",
        Primitive::Boolean(BooleanPrimitive::IfFalseIfTrue),
    ),
    mirror(
        "boolean:ifTrue:ifFail:",
        "ifTrue:",
        Primitive::Boolean(BooleanPrimitive::IfTrue),
    ),
    mirror(
        "boolean:ifFalse:ifFail:",
        "ifFalse:",
        Primitive::Boolean(BooleanPrimitive::IfFalse),
    ),
    mirror(
        "booleanNot:ifFail:",
        "not",
        Primitive::Boolean(BooleanPrimitive::Not),
    ),
    mirror(
        "boolean:logicalAnd:ifFail:",
        "&",
        Primitive::Boolean(BooleanPrimitive::And),
    ),
    mirror(
        "boolean:and:ifFail:",
        "and:",
        Primitive::Boolean(BooleanPrimitive::And),
    ),
    mirror(
        "boolean:logicalOr:ifFail:",
        "|",
        Primitive::Boolean(BooleanPrimitive::Or),
    ),
    mirror(
        "boolean:or:ifFail:",
        "or:",
        Primitive::Boolean(BooleanPrimitive::Or),
    ),
    mirror(
        "booleanPrintString:ifFail:",
        "printString",
        Primitive::Boolean(BooleanPrimitive::PrintString),
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
        "symbolPrintString:ifFail:",
        "printString",
        Primitive::Symbol(SymbolPrimitive::PrintString),
    ),
    mirror(
        "characterAsInteger:ifFail:",
        "asInteger",
        Primitive::Character(CharacterPrimitive::AsInteger),
    ),
    mirror(
        "characterAsString:ifFail:",
        "asString",
        Primitive::Character(CharacterPrimitive::AsString),
    ),
    mirror(
        "character:equals:ifFail:",
        "=",
        Primitive::Character(CharacterPrimitive::Equal),
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
        "collection:add:ifFail:",
        "add:",
        Primitive::Collection(CollectionPrimitive::Add),
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
        "platformTranscript:ifFail:",
        "transcript",
        Primitive::Platform(PlatformPrimitive::Transcript),
    ),
    mirror(
        "platformObject:ifFail:",
        "Object",
        Primitive::Platform(PlatformPrimitive::ObjectClass),
    ),
    mirror(
        "platformArray:ifFail:",
        "Array",
        Primitive::Platform(PlatformPrimitive::ArrayClass),
    ),
    mirror(
        "platformOrderedCollection:ifFail:",
        "OrderedCollection",
        Primitive::Platform(PlatformPrimitive::OrderedCollectionClass),
    ),
    mirror(
        "platformString:ifFail:",
        "String",
        Primitive::Platform(PlatformPrimitive::StringClass),
    ),
    mirror(
        "platformSymbol:ifFail:",
        "Symbol",
        Primitive::Platform(PlatformPrimitive::SymbolClass),
    ),
    mirror(
        "platformCollections:ifFail:",
        "collections",
        Primitive::Platform(PlatformPrimitive::Collections),
    ),
    mirror(
        "platformList:ifFail:",
        "List",
        Primitive::Platform(PlatformPrimitive::List),
    ),
    mirror(
        "platform:show:ifFail:",
        "show:",
        Primitive::Platform(PlatformPrimitive::Show),
    ),
    mirror(
        "platformNewLine:ifFail:",
        "newLine",
        Primitive::Platform(PlatformPrimitive::NewLine),
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
