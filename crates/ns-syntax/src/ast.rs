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

/// The opening and closing token of a bracketed construct, which an
/// editor folds between. `close` is `None` when the source has no closing
/// token there and the parser recovered without one; the construct then
/// ends where its node's span ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delimiters {
    pub open: Span,
    pub close: Option<Span>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassBody {
    pub slots: Vec<SlotDeclaration>,
    pub initializer_expressions: Vec<Statement>, // instance side only; empty on class side
    pub methods: Vec<MethodDeclaration>,
    pub nested_classes: Vec<ClassDeclaration>,
    /// The instance side's first `(` (the header's) and last `)` (the
    /// members'), or the class side's `( )`; `None` for a mixin
    /// application ended by `.`.
    pub delimiters: Option<Delimiters>,
    /// The header's slot section, `| |` or `|| ||`; `None` without one and
    /// on a class side.
    pub slot_delimiters: Option<Delimiters>,
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
    /// §6.3.2: declared in a simultaneous slot clause, `|| ... ||`, whose
    /// initializers run as futures resolved at the clause's end
    /// (`docs/design/object-model.md`, "Simultaneous slots"). Never set on a
    /// lazy slot, which stands in no clause.
    pub simultaneous: bool,
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
    /// The body's `( )`.
    pub body_delimiters: Option<Delimiters>,
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
    /// The temporaries' `| |`; `None` without any.
    pub temporaries_delimiters: Option<Delimiters>,
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

/// A chain of messages, `a b c + d …`, nests a send per message through
/// its receivers, as deep as the source is long: the parser builds it in a
/// loop, and no depth guard bounds it, since every length is valid. Each
/// send's receiver chain is therefore dropped here link by link, so a
/// dropped chain never recurses deeper than the bracketed nesting the
/// depth guard does bound. An argument's own chain is its sends' to drop.
impl Drop for Send {
    fn drop(&mut self) {
        let mut receiver = std::mem::replace(&mut self.receiver, placeholder(self.span));
        while let Expression::Send(mut link) = receiver {
            receiver = std::mem::replace(&mut link.receiver, placeholder(link.span));
        }
    }
}

impl Send {
    /// The send's receiver and message, taken out of it: a send owns its
    /// drop, so its fields cannot be moved out of it by a pattern.
    pub fn into_receiver_and_message(mut self) -> (Expression, MessageSend) {
        let receiver = std::mem::replace(&mut self.receiver, placeholder(self.span));
        let message = MessageSend {
            selector: std::mem::replace(&mut self.message.selector, Selector::Unary(String::new())),
            arguments: std::mem::take(&mut self.message.arguments),
            span: self.message.span,
        };
        (receiver, message)
    }

    /// This send and every send down its receivers, outermost first, and
    /// the receiver the chain starts from, which is no send. A walk over
    /// the tree takes a chain of messages from here rather than by
    /// recursing into each receiver: the chain is as deep as the source is
    /// long (see the `Drop` impl).
    pub fn receiver_chain(&self) -> (Vec<&Send>, &Expression) {
        let mut links = vec![self];
        let mut receiver = &self.receiver;
        while let Expression::Send(link) = receiver {
            links.push(link);
            receiver = &link.receiver;
        }
        (links, receiver)
    }
}

/// What stands in a send's receiver while the chain is taken apart.
fn placeholder(span: Span) -> Expression {
    Expression::Literal(Literal::Nil { span })
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
    /// The block's `[ ]`; `None` for a block the compiler builds.
    pub delimiters: Option<Delimiters>,
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
    /// §5.1.9's pattern literal, `<...>`, kept as its source text
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

    /// `1 + 1 + …` is valid at any length, and its chain nests a send per
    /// `+`: 20,000 of them overflowed a 1 MiB stack when dropped.
    #[test]
    fn a_long_chain_of_sends_drops_on_a_small_stack() {
        const LINKS: u32 = 100_000;
        const STACK_BYTES: usize = 256 * 1024;
        std::thread::Builder::new()
            .stack_size(STACK_BYTES)
            .spawn(|| {
                let mut chain = Expression::Literal(Literal::Nil {
                    span: Span::new(0, 0),
                });
                for link in 0..LINKS {
                    let span = Span::new(0, link);
                    chain = Expression::Send(Box::new(Send {
                        receiver: chain,
                        message: MessageSend {
                            selector: Selector::Unary("link".to_string()),
                            arguments: Vec::new(),
                            span,
                        },
                        span,
                    }));
                }
                drop(chain);
            })
            .expect("spawn the dropping thread")
            .join()
            .expect("the chain drops on a small stack");
    }
}
