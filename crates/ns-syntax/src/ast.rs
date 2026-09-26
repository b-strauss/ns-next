//! Abstract syntax tree for a Newspeak3 compilation unit.

use crate::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub language_id: String, // e.g. "Newspeak3"
    pub category: String,    // the quoted category string, contents only
    pub root_class: ClassDeclaration,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessModifier {
    Public,
    Protected,
    Private,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassDeclaration {
    pub access: Option<AccessModifier>,
    pub name: Identifier,
    pub factory: Option<MessagePattern>, // absent form: `class C = Super (...)`
    pub superclass: Option<SuperclassClause>,
    /// The `<:` clauses of a mixin application (§6.2), in written order; empty
    /// for a declaration with no `<:` at all. Each has the same shape as
    /// `superclass`, because §6.2's suffix is `(mixinOperator inheritanceClause)+`
    /// over §6.1's very production.
    pub mixin_applications: Vec<SuperclassClause>,
    pub class_comment: Option<String>,
    pub instance_side: ClassBody,
    pub class_side: Option<ClassBody>, // the `: ( ... )` block
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassBody {
    pub slots: Vec<SlotDeclaration>,
    pub initializer_expressions: Vec<Statement>, // instance side only; empty on class side
    pub methods: Vec<MethodDeclaration>,
    pub nested_classes: Vec<ClassDeclaration>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SuperclassClause {
    pub prefix: Option<SuperclassPrefix>,
    pub receiver: Identifier,       // the unaryMsg receiver identifier
    pub messages: Vec<MessageSend>, // trailing `message?` (0 or more sends)
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SuperclassPrefix {
    SelfPrefix,
    SuperPrefix,
    Outer(Identifier),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlotDeclaration {
    pub access: Option<AccessModifier>,
    pub name: Identifier,
    pub mutability: SlotMutability,
    pub initializer: Option<Expression>,
    /// §6.3.3: declared `lazy` in a class body -- initialized on first read
    /// of its getter, not by the instance initializer. Never set on a header
    /// slot or a temporary, which the grammar gives no `lazy` form.
    pub lazy: bool,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotMutability {
    /// `name = expression.`  immutable, initialized
    Immutable,
    /// `name ::= expression.`  mutable, initialized
    MutableInitialized,
    /// `name.`  mutable, uninitialized
    MutableBare,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodDeclaration {
    pub access: Option<AccessModifier>,
    pub pattern: MessagePattern,
    pub body: CodeBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessagePattern {
    Unary {
        selector: Identifier,
    },
    Binary {
        selector: Identifier,
        parameter: Identifier,
    },
    Keyword {
        parts: Vec<(Identifier, Identifier)>,
    }, // (keyword-without-colon, parameter)
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeBody {
    pub temporaries: Vec<SlotDeclaration>,
    pub statements: Vec<Statement>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Expression(Expression),
    Return(Expression), // `^ expression`
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    Identifier(Identifier),
    Literal(Literal),
    /// `receiver selector: argument ...`, `receiver + argument`, `receiver
    /// unary`
    Send(Box<Send>),
    /// `receiver msg1; msg2; msg3`
    Cascade(Box<Cascade>),
    /// `name:: value`, a setter send at an expression's head (§5.4.3)
    Assign {
        target: Identifier,
        value: Box<Expression>,
        span: Span,
    },
    Block(Box<BlockExpression>),
    /// `{ a. b. c }`
    DynamicArray {
        elements: Vec<Expression>,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Send {
    pub receiver: Expression,
    pub message: MessageSend,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageSend {
    pub selector: Selector,
    pub arguments: Vec<Expression>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cascade {
    pub receiver: Expression, // the receiver of the first message
    pub first: MessageSend,
    pub rest: Vec<MessageSend>, // each `; message`
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    Unary(String),
    Binary(String),
    Keyword(String), // full selector with colons, e.g. "at:put:"
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockExpression {
    pub parameters: Vec<Identifier>,
    pub body: CodeBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Integer {
        text: String,
        span: Span,
    }, // keep raw text; runtime parses/validates
    Float {
        text: String,
        span: Span,
    },
    String {
        value: String,
        span: Span,
    }, // unescaped contents
    Symbol {
        value: String,
        span: Span,
    }, // without the leading '#'
    Boolean {
        value: bool,
        span: Span,
    },
    Nil {
        span: Span,
    },
    /// `#( ... )` and `{ ... }` literal-array forms that contain only literals
    Array {
        elements: Vec<Literal>,
        span: Span,
    },
    /// `<...>` annotation/pattern literal — kept raw in Milestone 1
    Pattern {
        text: String,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identifier {
    pub name: String,
    pub span: Span,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_compare_by_value() {
        let first = Literal::Nil {
            span: Span::new(0, 3),
        };
        let second = Literal::Nil {
            span: Span::new(0, 3),
        };
        assert_eq!(first, second);
    }
}
