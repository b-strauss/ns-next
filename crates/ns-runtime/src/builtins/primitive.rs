//! The enumerated primitives: every native operation of the builtin classes,
//! named rather than selected by its selector's text, so that a lookup can
//! bind one and a later JIT tier can recognize it and emit it inline
//! (`docs/design/dispatch.md`, "Primitives").

use ns_syntax::ast;

use crate::hash::IdentityMap;
use crate::mixin::{MethodEntry, PrimitiveMethod};
use crate::types::BuiltinClass;
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

/// One selector a builtin class answers natively.
#[derive(Debug, Clone, Copy)]
pub struct PrimitiveSelector {
    pub selector: &'static str,
    pub access: ast::AccessModifier,
    pub primitive: Primitive,
}

const fn public(selector: &'static str, primitive: Primitive) -> PrimitiveSelector {
    PrimitiveSelector {
        selector,
        access: ast::AccessModifier::Public,
        primitive,
    }
}

// No remaining family list uses this once `Object`'s moved into `Kernel.ns`
// (Task 5); it stays for the families still ahead until Task 13 deletes the
// per-family lists (and this) entirely.
#[allow(dead_code)]
const fn protected(selector: &'static str, primitive: Primitive) -> PrimitiveSelector {
    PrimitiveSelector {
        selector,
        access: ast::AccessModifier::Protected,
        primitive,
    }
}

// `Object`'s methods moved into `Kernel.ns` (Task 5): each is now a kernel
// primitive method whose body forwards to the mirror row of the same
// `Primitive`. `newspeak-spec.pdf` §5.5 is why `doesNotUnderstand:` is
// `protected` there -- an explicit-receiver send of it finds nothing through
// `lookupPublic` and falls to the access-blind "defined for" walk that
// invokes it for the failing send itself.
const OBJECT: &[PrimitiveSelector] = &[];

// `Boolean`'s methods moved into `Kernel.ns` (Task 6), inherited by `True`
// and `False`, which declare none of their own.
const BOOLEAN: &[PrimitiveSelector] = &[];

// `Integer`'s methods moved into `Kernel.ns` (Task 7).
const INTEGER: &[PrimitiveSelector] = &[];

// `Float`'s methods moved into `Kernel.ns` (Task 8).
const FLOAT: &[PrimitiveSelector] = &[];

const STRING: &[PrimitiveSelector] = &[
    public("size", Primitive::String(StringPrimitive::Size)),
    public(",", Primitive::String(StringPrimitive::Concatenate)),
    public("=", Primitive::String(StringPrimitive::Equal)),
    public("asSymbol", Primitive::String(StringPrimitive::AsSymbol)),
    public(
        "printString",
        Primitive::String(StringPrimitive::PrintString),
    ),
];

const SYMBOL: &[PrimitiveSelector] = &[
    public("asString", Primitive::Symbol(SymbolPrimitive::AsString)),
    public("=", Primitive::Symbol(SymbolPrimitive::Equal)),
    public(
        "printString",
        Primitive::Symbol(SymbolPrimitive::PrintString),
    ),
];

const CHARACTER: &[PrimitiveSelector] = &[
    public(
        "asInteger",
        Primitive::Character(CharacterPrimitive::AsInteger),
    ),
    public(
        "asString",
        Primitive::Character(CharacterPrimitive::AsString),
    ),
    public("=", Primitive::Character(CharacterPrimitive::Equal)),
];

const ARRAY: &[PrimitiveSelector] = &[
    public("do:", Primitive::Collection(CollectionPrimitive::Enumerate)),
    public(
        "at:put:",
        Primitive::Collection(CollectionPrimitive::ElementAtPut),
    ),
    public("size", Primitive::Collection(CollectionPrimitive::Size)),
    public("at:", Primitive::Collection(CollectionPrimitive::ElementAt)),
];

const ORDERED_COLLECTION: &[PrimitiveSelector] = &[
    public("do:", Primitive::Collection(CollectionPrimitive::Enumerate)),
    public("add:", Primitive::Collection(CollectionPrimitive::Add)),
    public("size", Primitive::Collection(CollectionPrimitive::Size)),
    public("at:", Primitive::Collection(CollectionPrimitive::ElementAt)),
];

const BLOCK: &[PrimitiveSelector] = &[
    public("value", Primitive::Block(BlockPrimitive::Value)),
    public("value:", Primitive::Block(BlockPrimitive::Value)),
    public("value:value:", Primitive::Block(BlockPrimitive::Value)),
    public(
        "value:value:value:",
        Primitive::Block(BlockPrimitive::Value),
    ),
    public("numArgs", Primitive::Block(BlockPrimitive::ArgumentCount)),
    public("whileTrue:", Primitive::Block(BlockPrimitive::WhileTrue)),
    public("whileFalse:", Primitive::Block(BlockPrimitive::WhileFalse)),
];

const PLATFORM: &[PrimitiveSelector] = &[
    public(
        "transcript",
        Primitive::Platform(PlatformPrimitive::Transcript),
    ),
    public(
        "Object",
        Primitive::Platform(PlatformPrimitive::ObjectClass),
    ),
    public("Array", Primitive::Platform(PlatformPrimitive::ArrayClass)),
    public(
        "OrderedCollection",
        Primitive::Platform(PlatformPrimitive::OrderedCollectionClass),
    ),
    public(
        "String",
        Primitive::Platform(PlatformPrimitive::StringClass),
    ),
    public(
        "Symbol",
        Primitive::Platform(PlatformPrimitive::SymbolClass),
    ),
    public(
        "collections",
        Primitive::Platform(PlatformPrimitive::Collections),
    ),
    public("List", Primitive::Platform(PlatformPrimitive::List)),
    public("show:", Primitive::Platform(PlatformPrimitive::Show)),
    public("newLine", Primitive::Platform(PlatformPrimitive::NewLine)),
];

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
}

impl MirrorEntry {
    /// How many arguments the reported selector takes: one for a binary
    /// selector, one per colon for a keyword selector, none for a unary one.
    pub fn leaf_arity(&self) -> usize {
        let binary = !self
            .reported
            .starts_with(|character: char| character.is_alphabetic());
        if binary {
            1
        } else {
            self.reported.matches(':').count()
        }
    }
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
    }
}

/// The complete leaf table the VM mirror answers (`docs/design/core-library.md`,
/// "The VM mirror is the whole Rust boundary"; the plan's "The mirror
/// table" names each row's index). `Array` and `OrderedCollection` share
/// rows 49-53, as they share their primitives today; each kernel class
/// declares only the selectors its own list above names.
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

/// The selectors `kind` answers natively, in the order its module lists them.
pub fn primitives_of(kind: BuiltinClass) -> &'static [PrimitiveSelector] {
    match kind {
        BuiltinClass::Object => OBJECT,
        BuiltinClass::Boolean | BuiltinClass::True | BuiltinClass::False => BOOLEAN,
        BuiltinClass::Integer => INTEGER,
        BuiltinClass::Float => FLOAT,
        BuiltinClass::String => STRING,
        BuiltinClass::Symbol => SYMBOL,
        BuiltinClass::Character => CHARACTER,
        BuiltinClass::Array => ARRAY,
        BuiltinClass::OrderedCollection => ORDERED_COLLECTION,
        BuiltinClass::Block => BLOCK,
        BuiltinClass::Platform => PLATFORM,
        BuiltinClass::Nil => &[],
        // The mirror's entries come from `MIRROR`, not from a
        // `PrimitiveSelector` list -- `method_table` special-cases it below.
        BuiltinClass::VmMirror => &[],
    }
}

/// The method table the bootstrap gives `kind`'s mixin: one
/// `MethodEntry::Primitive` per selector `kind` answers natively. The VM
/// mirror's table comes from `MIRROR` directly, one row per entry, rather
/// than from `primitives_of`, whose per-family lists it is not one of.
pub fn method_table(kind: BuiltinClass) -> IdentityMap<Symbol, MethodEntry> {
    if kind == BuiltinClass::VmMirror {
        return MIRROR
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
            .collect();
    }
    primitives_of(kind)
        .iter()
        .map(|entry| {
            (
                Symbol::intern(entry.selector),
                MethodEntry::Primitive(PrimitiveMethod {
                    primitive: entry.primitive,
                    access: entry.access,
                }),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A method table holds one entry per selector, so a list that names a
    /// selector twice would silently keep only one of its primitives.
    #[test]
    fn no_builtin_lists_a_selector_twice() {
        for kind in BuiltinClass::ALL {
            let mut seen = HashSet::new();
            for entry in primitives_of(kind) {
                assert!(
                    seen.insert(entry.selector),
                    "{kind:?} lists `{}` twice",
                    entry.selector
                );
            }
        }
    }

    /// `Nil` has no natives of its own today; the VM mirror's come from
    /// `MIRROR` rather than `primitives_of` (`method_table`'s special case);
    /// `Object`'s methods moved into `Kernel.ns` (Task 5), `Boolean`'s (Task
    /// 6, inherited by `True` and `False`, which declare none of their
    /// own), `Integer`'s (Task 7) and `Float`'s (Task 8), so their lists are
    /// empty too; every other builtin still has a `primitives_of` list.
    #[test]
    fn every_builtin_but_nil_the_mirror_object_boolean_integer_and_float_has_primitives() {
        for kind in BuiltinClass::ALL {
            assert_eq!(
                primitives_of(kind).is_empty(),
                matches!(
                    kind,
                    BuiltinClass::Nil
                        | BuiltinClass::VmMirror
                        | BuiltinClass::Object
                        | BuiltinClass::Boolean
                        | BuiltinClass::True
                        | BuiltinClass::False
                        | BuiltinClass::Integer
                        | BuiltinClass::Float
                ),
                "{kind:?}"
            );
        }
    }

    /// `Integer`'s `+` moved into `Kernel.ns` (Task 7): the leaf it forwards
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
