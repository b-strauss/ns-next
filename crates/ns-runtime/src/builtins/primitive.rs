//! The enumerated primitives: every native operation of the builtin classes,
//! named rather than selected by its selector's text, so that a lookup can
//! bind one and a later JIT tier can recognize it and emit it inline
//! (`docs/design/dispatch.md`, "3a — Primitives are method-table entries").

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

const OBJECT: &[PrimitiveSelector] = &[
    public("class", Primitive::Object(ObjectPrimitive::Class)),
    public("==", Primitive::Object(ObjectPrimitive::Identical)),
    public("=", Primitive::Object(ObjectPrimitive::Equal)),
    public("hash", Primitive::Object(ObjectPrimitive::Hash)),
    public(
        "printString",
        Primitive::Object(ObjectPrimitive::PrintString),
    ),
    public(
        "doesNotUnderstand:",
        Primitive::Object(ObjectPrimitive::DoesNotUnderstand),
    ),
];

const BOOLEAN: &[PrimitiveSelector] = &[
    public(
        "ifTrue:ifFalse:",
        Primitive::Boolean(BooleanPrimitive::IfTrueIfFalse),
    ),
    public(
        "ifFalse:ifTrue:",
        Primitive::Boolean(BooleanPrimitive::IfFalseIfTrue),
    ),
    public("ifTrue:", Primitive::Boolean(BooleanPrimitive::IfTrue)),
    public("ifFalse:", Primitive::Boolean(BooleanPrimitive::IfFalse)),
    public("not", Primitive::Boolean(BooleanPrimitive::Not)),
    public("&", Primitive::Boolean(BooleanPrimitive::And)),
    public("and:", Primitive::Boolean(BooleanPrimitive::And)),
    public("|", Primitive::Boolean(BooleanPrimitive::Or)),
    public("or:", Primitive::Boolean(BooleanPrimitive::Or)),
    public(
        "printString",
        Primitive::Boolean(BooleanPrimitive::PrintString),
    ),
];

const INTEGER: &[PrimitiveSelector] = &[
    public("+", Primitive::Integer(IntegerPrimitive::Add)),
    public("-", Primitive::Integer(IntegerPrimitive::Subtract)),
    public("*", Primitive::Integer(IntegerPrimitive::Multiply)),
    public("//", Primitive::Integer(IntegerPrimitive::FloorDivide)),
    public("<", Primitive::Integer(IntegerPrimitive::Less)),
    public(">", Primitive::Integer(IntegerPrimitive::Greater)),
    public("<=", Primitive::Integer(IntegerPrimitive::LessOrEqual)),
    public(">=", Primitive::Integer(IntegerPrimitive::GreaterOrEqual)),
    public("=", Primitive::Integer(IntegerPrimitive::Equal)),
    public("negated", Primitive::Integer(IntegerPrimitive::Negated)),
    public("abs", Primitive::Integer(IntegerPrimitive::Absolute)),
    public(
        "printString",
        Primitive::Integer(IntegerPrimitive::PrintString),
    ),
    public(
        "asString",
        Primitive::Integer(IntegerPrimitive::PrintString),
    ),
];

const FLOAT: &[PrimitiveSelector] = &[
    public("+", Primitive::Float(FloatPrimitive::Add)),
    public("-", Primitive::Float(FloatPrimitive::Subtract)),
    public("*", Primitive::Float(FloatPrimitive::Multiply)),
    public("/", Primitive::Float(FloatPrimitive::Divide)),
    public("<", Primitive::Float(FloatPrimitive::Less)),
    public(">", Primitive::Float(FloatPrimitive::Greater)),
    public("=", Primitive::Float(FloatPrimitive::Equal)),
    public("printString", Primitive::Float(FloatPrimitive::PrintString)),
    public("asString", Primitive::Float(FloatPrimitive::PrintString)),
];

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
    }
}

/// The method table the bootstrap gives `kind`'s mixin: one
/// `MethodEntry::Primitive` per selector `kind` answers natively.
pub fn method_table(kind: BuiltinClass) -> IdentityMap<Symbol, MethodEntry> {
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

    /// `Nil` has no natives of its own today; every other builtin does.
    #[test]
    fn every_builtin_but_nil_has_primitives() {
        for kind in BuiltinClass::ALL {
            assert_eq!(
                primitives_of(kind).is_empty(),
                kind == BuiltinClass::Nil,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn integer_addition_is_its_own_primitive() {
        let entry = primitives_of(BuiltinClass::Integer)
            .iter()
            .find(|entry| entry.selector == "+")
            .expect("Integer lists `+`");
        assert_eq!(entry.primitive, Primitive::Integer(IntegerPrimitive::Add));
        assert_eq!(entry.access, ast::AccessModifier::Public);
    }
}
