//! Abstract syntax tree for a Newspeak3 compilation unit.

use crate::comment::CommentKind;
use crate::span::Span;
use crate::token::TokenKind;

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// The language id that opens the module, e.g. `Newspeak3`; `None` when
    /// the source leaves it out.
    pub language_id: Option<Identifier>,
    /// The category string before the root class; `None` when the source
    /// leaves it out.
    pub category: Option<Category>,
    pub root_class: ClassDeclaration,
    /// The comments attached to the module, in source order (the rule is
    /// `attach.rs`'s).
    pub comments: Vec<AttachedComment>,
    pub span: Span,
}

/// The module's category string (§8.1's `classCategory`).
#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    /// The string unescaped, quotes left off.
    pub value: String,
    /// The string as written, quotes included.
    pub text: String,
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
    /// The type parameters after the name, `<[K, V]>`, `[E]` or both, as
    /// written: one annotation may hold two bracket groups.
    pub type_parameters: Option<TypeAnnotation>,
    pub factory: Option<MessagePattern>, // absent form: `class C = Super (...)`
    /// The factory's `^<Type>`, the caret left out.
    pub factory_return_type: Option<TypeAnnotation>,
    /// The factory's `where { ... }` clause.
    pub factory_where_clause: Option<TypeAnnotation>,
    pub superclass: Option<SuperclassClause>,
    /// The `<:` clauses of a mixin application (§6.2), in written order; empty
    /// for a declaration with no `<:` at all. Each has the same shape as
    /// `superclass`, because §6.2's suffix is `(mixinOperator inheritanceClause)+`
    /// over §6.1's very production.
    pub mixin_applications: Vec<SuperclassClause>,
    pub instance_side: ClassBody,
    pub class_side: Option<ClassBody>, // the `: ( ... )` block
    /// The comments attached to the declaration, in source order (the rule is
    /// `attach.rs`'s).
    pub comments: Vec<AttachedComment>,
    pub span: Span,
}

impl ClassDeclaration {
    /// The class comment: the first comment in the header, before any slot
    /// or initializer expression -- one right after the header's `(`
    /// (`Inner(1)` on the instance side, before the slot section's bar and
    /// before the first initializer expression), else one leading the first
    /// header slot, else, the slot section declaring none, one leading the
    /// first initializer expression. A `///` after the `(` on its line is a
    /// remark on that line, not the class comment. `None` for a mixin
    /// application ended by `.`, which has no header.
    pub fn class_comment(&self) -> Option<&AttachedComment> {
        let side = &self.instance_side;
        side.delimiters?;
        let first_initializer = side.initializer_expressions.first();
        // `Inner(1)` follows the initializer expressions when no slot bar
        // stands between them and the `(`.
        let after_open = side.comments.iter().filter(|comment| {
            comment.anchor == Anchor::Inner(1)
                && first_initializer
                    .is_none_or(|statement| comment.span.end <= statement.span.start)
        });
        let leading_the_first_slot = side
            .header_slots
            .first()
            .map_or(&[][..], |slot| slot.comments.as_slice());
        let leading_the_first_initializer = match first_initializer {
            Some(statement) if side.header_slots.is_empty() => statement.comments.as_slice(),
            _ => &[],
        };
        let leading = leading_the_first_slot
            .iter()
            .chain(leading_the_first_initializer)
            .filter(|comment| comment.anchor == Anchor::Before);
        after_open.chain(leading).find(|comment| {
            !(comment.kind == CommentKind::Line && comment.placement == Placement::EndOfLine)
        })
    }
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

/// A member of one side of a class, `sideDecl`'s `nestedClassDecl | lazySlotDecl |
/// methodDecl` (§6.3); a class side's `classSideDecl` holds methods only.
/// A nested class is boxed: it is several times the size of the other two.
#[derive(Debug, Clone, PartialEq)]
pub enum Member {
    Class(Box<ClassDeclaration>),
    LazySlot(SlotDeclaration),
    Method(MethodDeclaration),
}

/// One side of a class. The instance side is the header's group (its slots
/// and initializer expressions) and the members' group; the class side,
/// `: ( ... )`, has members only, every one a method.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassBody {
    /// The header's slots (instance side only), in written order.
    pub header_slots: Vec<SlotDeclaration>,
    pub initializer_expressions: Vec<Statement>, // instance side only; empty on class side
    /// The side's members in source order.
    pub members: Vec<Member>,
    /// The instance side's first `(` (the header's) and last `)` (the
    /// members'), or the class side's `( )`; `None` for a mixin
    /// application ended by `.`.
    pub delimiters: Option<Delimiters>,
    /// The instance side's members' `( )`, after the header's; `None` on a
    /// class side, for a mixin application ended by `.`, and when recovery
    /// found no `(` there.
    pub members_delimiters: Option<Delimiters>,
    /// The header's slot section, `| |` or `|| ||`; `None` without one and
    /// on a class side.
    pub slot_delimiters: Option<Delimiters>,
    /// The comments attached to the side, in source order (the rule is
    /// `attach.rs`'s).
    pub comments: Vec<AttachedComment>,
    pub span: Span,
}

impl ClassBody {
    /// The header's slots, then the lazy slots in source order: the order
    /// slot indices and image rows have always used.
    pub fn slots(&self) -> impl Iterator<Item = &SlotDeclaration> {
        self.header_slots.iter().chain(self.lazy_slots())
    }

    /// The methods, in source order.
    pub fn methods(&self) -> impl Iterator<Item = &MethodDeclaration> {
        self.members.iter().filter_map(|member| match member {
            Member::Method(method) => Some(method),
            _ => None,
        })
    }

    /// The nested classes, in source order.
    pub fn nested_classes(&self) -> impl Iterator<Item = &ClassDeclaration> {
        self.members.iter().filter_map(|member| match member {
            Member::Class(class) => Some(class.as_ref()),
            _ => None,
        })
    }

    /// The lazy slots, in source order.
    pub fn lazy_slots(&self) -> impl Iterator<Item = &SlotDeclaration> {
        self.members.iter().filter_map(|member| match member {
            Member::LazySlot(slot) => Some(slot),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SuperclassClause {
    pub prefix: Option<SuperclassPrefix>,
    pub receiver: Identifier, // the unaryMsg receiver identifier
    /// Type arguments written right after the receiver, `Collection[E]`.
    pub receiver_type_arguments: Option<TypeAnnotation>,
    pub messages: Vec<MessageSend>, // trailing `message?` (0 or more sends)
    /// The comments attached to the clause, in source order (the rule is
    /// `attach.rs`'s).
    pub comments: Vec<AttachedComment>,
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
    /// The `<Type>` after the name.
    pub type_annotation: Option<TypeAnnotation>,
    pub mutability: SlotMutability,
    pub initializer: Option<Expression>,
    /// §6.3.3: declared `lazy` in a class body -- initialized on first read
    /// of its getter, not by the instance initializer. Never set on a header
    /// slot or a temporary, which the grammar gives no `lazy` form.
    pub lazy: bool,
    /// A lazy slot's access modifier was written before `lazy` (`public
    /// lazy x`), not after it (`lazy public x`): the written order, which
    /// means nothing. `false` without an access modifier and on every slot
    /// that is not lazy.
    pub access_before_lazy: bool,
    /// §6.3.2: declared in a simultaneous slot clause, `|| ... ||`, whose
    /// initializers run as futures resolved at the clause's end
    /// (`docs/design/object-model.md`, "Simultaneous slots"). Never set on a
    /// lazy slot, which stands in no clause.
    pub simultaneous: bool,
    /// The comments attached to the slot, in source order (the rule is
    /// `attach.rs`'s).
    pub comments: Vec<AttachedComment>,
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
    /// The `^<Type>` after the pattern, the caret left out.
    pub return_type: Option<TypeAnnotation>,
    /// The `where { ... }` clause after the pattern.
    pub where_clause: Option<TypeAnnotation>,
    pub body: CodeBody,
    /// The body's `( )`.
    pub body_delimiters: Option<Delimiters>,
    /// The comments attached to the method, in source order (the rule is
    /// `attach.rs`'s).
    pub comments: Vec<AttachedComment>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessagePattern {
    Unary {
        selector: Identifier,
        /// What follows the selector: the type parameters, `List[E]`, and a
        /// `<Type>` after them, in one annotation as written, which may
        /// therefore hold two bracket groups.
        type_parameters: Option<TypeAnnotation>,
    },
    Binary {
        selector: Identifier,
        parameter: Parameter,
    },
    Keyword {
        parts: Vec<KeywordPart>,
    },
}

/// A parameter of a message pattern or a block, and the `<Type>` after it.
#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub name: Identifier,
    pub type_annotation: Option<TypeAnnotation>,
}

/// One keyword of a keyword pattern and its parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct KeywordPart {
    /// The keyword without its colon.
    pub keyword: Identifier,
    pub parameter: Parameter,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeBody {
    pub temporaries: Vec<SlotDeclaration>,
    /// The temporaries' `| |`; `None` without any.
    pub temporaries_delimiters: Option<Delimiters>,
    pub statements: Vec<Statement>,
    /// The comments attached to the body, in source order (the rule is
    /// `attach.rs`'s).
    pub comments: Vec<AttachedComment>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    /// The comments attached to the statement, in source order (the rule is
    /// `attach.rs`'s).
    pub comments: Vec<AttachedComment>,
    /// From the statement's first token (`^` for a return) to its last,
    /// the separating period excluded.
    pub span: Span,
}

impl Statement {
    /// A statement of `expression` alone, spanning it.
    pub fn of_expression(expression: Expression) -> Statement {
        Statement {
            span: expression.span,
            kind: StatementKind::Expression(expression),
            comments: Vec::new(),
        }
    }

    /// A statement returning `expression`, spanning the expression: one
    /// the compiler builds, which has no `^` in the source.
    pub fn of_return(expression: Expression) -> Statement {
        Statement {
            span: expression.span,
            kind: StatementKind::Return(expression),
            comments: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatementKind {
    Expression(Expression),
    Return(Expression), // `^ expression`
}

/// An expression: its shape, and what surrounds the shape in the source
/// without changing what it means.
#[derive(Debug, Clone, PartialEq)]
pub struct Expression {
    pub kind: ExpressionKind,
    /// The parentheses around the shape, innermost first.
    pub parentheses: Vec<Parenthesis>,
    /// Value-site type arguments written right after the shape, before any
    /// `)` around it: `Box[Integer]`.
    pub type_arguments: Option<TypeAnnotation>,
    /// The parser made this node up; nothing in the source spells it (the
    /// implicit receiver of a send).
    pub synthesized: bool,
    /// The comments attached to this expression, in source order (the rule is
    /// `attach.rs`'s).
    pub comments: Vec<AttachedComment>,
    /// The shape's span, widened over the parentheses.
    pub span: Span,
}

impl Expression {
    /// An expression of `kind` with nothing around it, spanning `span`.
    pub fn bare(kind: ExpressionKind, span: Span) -> Expression {
        Expression {
            kind,
            parentheses: Vec::new(),
            type_arguments: None,
            synthesized: false,
            comments: Vec::new(),
            span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parenthesis {
    pub delimiters: Delimiters,
    /// Type arguments written right after this `)`, `(Box)[Integer]`.
    pub type_arguments: Option<TypeAnnotation>,
}

/// A type annotation, kept as the tokens written: the specification gives
/// no type grammar and the annotations document only (§4.3, "optional type
/// annotation syntax based on the Strongtalk type system"). Nothing after
/// the parser reads one: the resolver and the compiler ignore every annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeAnnotation {
    /// Every token from the opening `<`, `[` or `where` through the closer
    /// that ends it, in order; a recovered annotation without its closer
    /// ends at the last token taken.
    pub tokens: Vec<TypeToken>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeToken {
    /// The lexer's kind of the token.
    pub kind: TokenKind,
    pub text: String,
}

/// A comment, kept in the AST at the node it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct AttachedComment {
    pub kind: CommentKind,
    /// `Line`: the text after `///` on each line of the block, trailing
    /// whitespace removed. `Block`: the comment's lines from `(*` through
    /// `*)`, trailing whitespace removed; continuation lines keep their own
    /// leading whitespace.
    pub lines: Vec<String>,
    /// `Block`: the column, in `char`s from the start of its line, at which
    /// `(*` stood; the printer shifts the continuation lines by the
    /// difference to where it prints `(*`. `Line`: 0.
    pub column: u32,
    pub anchor: Anchor,
    /// Where the comment stood on its line, which the printer keeps: an
    /// `After` comment printed on a line of its own (`OwnLine`) is read
    /// back as leading the next node (`print_module`).
    pub placement: Placement,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// Before the node, after whatever precedes it.
    Before,
    /// After the node, before whatever follows it.
    After,
    /// Inside the node, after its `n`-th own token and the commentable
    /// children that follow that token, before its next own token
    /// (`attach.rs` lists each node's own tokens).
    Inner(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Nothing but whitespace before it on its line (comments beside it
    /// count as nothing).
    OwnLine,
    /// Code before it on its line, a line break (or the end) after it
    /// (comments beside it count as nothing).
    EndOfLine,
    /// Code, or a delimiter, on both sides on the same line.
    Inline,
}

/// What an expression is, apart from what surrounds it.
#[derive(Debug, Clone, PartialEq)]
pub enum ExpressionKind {
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

impl ExpressionKind {
    /// The span of the shape alone, without the parentheses around it: the
    /// key of every side table, so a parenthesised name finds the binding
    /// the bare one would.
    pub fn span(&self) -> Span {
        match self {
            ExpressionKind::Identifier(identifier) => identifier.span,
            ExpressionKind::Literal(
                Literal::Integer { span, .. }
                | Literal::Float { span, .. }
                | Literal::String { span, .. }
                | Literal::Symbol { span, .. }
                | Literal::Boolean { span, .. }
                | Literal::Nil { span }
                | Literal::Pattern { span, .. },
            ) => *span,
            ExpressionKind::Send(send) => send.span,
            ExpressionKind::Cascade(cascade) => cascade.span,
            ExpressionKind::Assign { span, .. } => *span,
            ExpressionKind::Block(block) => block.span,
            ExpressionKind::DynamicArray { span, .. } => *span,
        }
    }
}

#[derive(Debug, PartialEq)]
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
        while let ExpressionKind::Send(mut link) = receiver.kind {
            receiver = std::mem::replace(&mut link.receiver, placeholder(link.span));
        }
    }
}

/// A chain of messages is cloned link by link, for the reason it is
/// dropped so (see the `Drop` impl).
impl Clone for Send {
    fn clone(&self) -> Send {
        let (links, start) = self.receiver_chain();
        let mut receiver = start.clone();
        let mut index = links.len();
        loop {
            index -= 1;
            let link = links[index];
            let send = Send {
                receiver,
                message: link.message.clone(),
                span: link.span,
            };
            let Some(outer) = index.checked_sub(1) else {
                return send;
            };
            // The envelope around this link is the receiver of the link
            // outside it.
            let envelope = &links[outer].receiver;
            receiver = Expression {
                kind: ExpressionKind::Send(Box::new(send)),
                parentheses: envelope.parentheses.clone(),
                type_arguments: envelope.type_arguments.clone(),
                synthesized: envelope.synthesized,
                comments: envelope.comments.clone(),
                span: envelope.span,
            };
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
            eventual: self.message.eventual,
            type_arguments: self.message.type_arguments.take(),
            comments: std::mem::take(&mut self.message.comments),
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
        while let ExpressionKind::Send(link) = &receiver.kind {
            links.push(link);
            receiver = &link.receiver;
        }
        (links, receiver)
    }
}

/// What stands in a send's receiver while the chain is taken apart.
fn placeholder(span: Span) -> Expression {
    Expression::bare(ExpressionKind::Literal(Literal::Nil { span }), span)
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageSend {
    pub selector: Selector,
    pub arguments: Vec<Expression>,
    /// Sent with `<-:` (§5.6). The runtime runs it as an ordinary send.
    pub eventual: bool,
    /// Type arguments written right after a unary selector, `foo bar[E]`.
    pub type_arguments: Option<TypeAnnotation>,
    /// The comments attached to the message, in source order (the rule is
    /// `attach.rs`'s).
    pub comments: Vec<AttachedComment>,
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
    pub parameters: Vec<Parameter>,
    pub body: CodeBody,
    /// The block's `[ ]`; `None` for a block the compiler builds.
    pub delimiters: Option<Delimiters>,
    /// The comments attached to the block, in source order (the rule is
    /// `attach.rs`'s).
    pub comments: Vec<AttachedComment>,
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
    /// `value` is unescaped; `text` is as written, quotes included
    /// (`'it''s'`, `"a"`).
    String {
        value: String,
        text: String,
        span: Span,
    },
    /// `value` is without the `#` and the quotes; `text` is as written
    /// (`#foo`, `#'a b'`, `#at:put:`).
    Symbol {
        value: String,
        text: String,
        span: Span,
    },
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
                let mut chain = Expression::bare(
                    ExpressionKind::Literal(Literal::Nil {
                        span: Span::new(0, 0),
                    }),
                    Span::new(0, 0),
                );
                for link in 0..LINKS {
                    let span = Span::new(0, link);
                    let send = ExpressionKind::Send(Box::new(Send {
                        receiver: chain,
                        message: MessageSend {
                            selector: Selector::Unary("link".to_string()),
                            arguments: Vec::new(),
                            eventual: false,
                            type_arguments: None,
                            comments: Vec::new(),
                            span,
                        },
                        span,
                    }));
                    chain = Expression::bare(send, span);
                }
                drop(chain);
            })
            .expect("spawn the dropping thread")
            .join()
            .expect("the chain drops on a small stack");
    }
}
