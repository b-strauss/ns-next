//! Abstract syntax tree for a Newspeak3 compilation unit.

use crate::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub language_id: String, // e.g. "Newspeak3"
    pub category: String,    // the quoted category string, contents only
    pub root_class: ClassDecl,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessModifier {
    Public,
    Protected,
    Private,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassDecl {
    pub access: Option<AccessModifier>,
    pub name: Ident,
    pub factory: Option<MessagePattern>, // absent form: `class C = Super (...)`
    pub superclass: Option<SuperclassClause>,
    pub class_comment: Option<String>,
    pub instance_side: ClassBody,
    pub class_side: Option<ClassBody>, // the `: ( ... )` block
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassBody {
    pub slots: Vec<SlotDecl>,
    pub init_exprs: Vec<Stmt>, // instance side only; empty on class side
    pub methods: Vec<MethodDecl>,
    pub nested_classes: Vec<ClassDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SuperclassClause {
    pub prefix: Option<SuperclassPrefix>,
    pub receiver: Ident,            // the unaryMsg receiver identifier
    pub messages: Vec<MessageSend>, // trailing `message?` (0 or more sends)
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SuperclassPrefix {
    SelfPrefix,
    SuperPrefix,
    Outer(Ident),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlotDecl {
    pub access: Option<AccessModifier>,
    pub name: Ident,
    pub mutability: SlotMutability,
    pub init: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotMutability {
    /// `name = expr.`  immutable, initialized
    Immutable,
    /// `name ::= expr.`  mutable, initialized
    MutableInit,
    /// `name.`  mutable, uninitialized
    MutableBare,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodDecl {
    pub access: Option<AccessModifier>,
    pub pattern: MessagePattern,
    pub body: CodeBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessagePattern {
    Unary { selector: Ident },
    Binary { selector: Ident, param: Ident },
    Keyword { parts: Vec<(Ident, Ident)> }, // (keyword-without-colon, param)
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeBody {
    pub temps: Vec<Ident>,
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Expr(Expr),
    Return(Expr), // `^ expr`
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Ident(Ident),
    Literal(Literal),
    /// `receiver selector: arg ...`, `receiver + arg`, `receiver unary`
    Send(Box<Send>),
    /// `receiver msg1; msg2; msg3`
    Cascade(Box<Cascade>),
    /// `lhs := rhs`  (lhs is always an Ident in Newspeak)
    Assign {
        target: Ident,
        value: Box<Expr>,
        span: Span,
    },
    Block(Box<BlockExpr>),
    /// `{ a. b. c }`
    DynamicArray {
        elements: Vec<Expr>,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Send {
    pub receiver: Expr,
    pub message: MessageSend,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageSend {
    pub selector: Selector,
    pub args: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cascade {
    pub receiver: Expr, // the receiver of the first message
    pub first: MessageSend,
    pub rest: Vec<MessageSend>, // each `; msg`
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    Unary(String),
    Binary(String),
    Keyword(String), // full selector with colons, e.g. "at:put:"
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockExpr {
    pub params: Vec<Ident>,
    pub body: CodeBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int {
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
    Char {
        value: char,
        span: Span,
    },
    Bool {
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
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_compare_by_value() {
        let a = Literal::Nil {
            span: Span::new(0, 3),
        };
        let b = Literal::Nil {
            span: Span::new(0, 3),
        };
        assert_eq!(a, b);
    }
}
