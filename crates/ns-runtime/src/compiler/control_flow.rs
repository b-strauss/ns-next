//! Compile-time recognition of the standard Smalltalk control-flow
//! "special selectors" for inlining. Pure recognition only -- no bytecode
//! emission lives here (see `lower.rs`'s `lower_if`/`lower_and_or`/
//! `lower_while`). See `docs/design/compiler.md`, "Control-flow inlining".

use std::collections::HashMap;

use ns_syntax::ast;
use ns_syntax::Span;

use crate::builtins::primitive::MirrorPrimitive;
use crate::bytecode::IntegerOperation;
use crate::types::Binding;

/// Whether `send` is an ordinary explicit-receiver send -- the gate in front
/// of `recognize`, since a `super` or implicit-receiver (lexical) send is
/// never inlined and has no receiver expression to walk.
///
/// Shared rather than repeated because three places must answer it
/// identically: `lower_send`, which lowers the send; `send_writes_register`,
/// which asks what a send can clobber before it; and `capture::Walker::send`,
/// which decides which block literals the analysis sees as real. A fourth
/// answer anywhere would let the analysis and the lowering disagree about
/// which blocks become closures.
pub(crate) fn is_ordinary_send(bindings: &HashMap<Span, Binding>, send: &ast::Send) -> bool {
    !matches!(
        bindings.get(&send.span),
        Some(Binding::SuperSend | Binding::LexicalSend { .. } | Binding::EnclosingObject { .. })
    )
}

pub(crate) enum InlineForm<'a> {
    /// `ifTrue:ifFalse:` and (arguments swapped) `ifFalse:ifTrue:`.
    IfTrueIfFalse(&'a ast::BlockExpression, &'a ast::BlockExpression),
    IfTrue(&'a ast::BlockExpression),
    IfFalse(&'a ast::BlockExpression),
    And(&'a ast::BlockExpression),
    Or(&'a ast::BlockExpression),
    WhileTrue {
        condition: &'a ast::BlockExpression,
        body: &'a ast::BlockExpression,
    },
    WhileFalse {
        condition: &'a ast::BlockExpression,
        body: &'a ast::BlockExpression,
    },
}

/// The gate's per-block condition: a syntactic block literal with no
/// parameters and no temporaries. A block with temporaries gets a fresh
/// activation (and fresh temporaries) per invocation, which a closure created
/// inside a loop body could observe; a register cannot reproduce that, so
/// such blocks keep their real-send semantics.
fn as_inlinable_block(expression: &ast::Expression) -> Option<&ast::BlockExpression> {
    match &expression.kind {
        ast::ExpressionKind::Block(block)
            if block.parameters.is_empty() && block.body.temporaries.is_empty() =>
        {
            Some(block)
        }
        _ => None,
    }
}

/// Only ever offered ordinary explicit-receiver sends (see `lower_send`);
/// looks at the message shape and the argument/receiver expressions only.
pub(crate) fn recognize(send: &ast::Send) -> Option<InlineForm<'_>> {
    let ast::Selector::Keyword(selector) = &send.message.selector else {
        return None;
    };
    let arguments = &send.message.arguments;
    // The resolver/parser guarantee `arguments.len()` matches the selector's
    // own colon count; each arm below indexes `arguments` at a fixed position
    // with the arity only implicit in the selector string it matched on. Name
    // the invariant so a mismatch surfaces as this assertion, not a bare
    // slice-index panic.
    let expected_arity = selector.matches(':').count();
    debug_assert_eq!(
        arguments.len(),
        expected_arity,
        "internal: keyword selector `{selector}` expects {expected_arity} argument(s), got {}",
        arguments.len()
    );
    match selector.as_str() {
        "ifTrue:ifFalse:" => Some(InlineForm::IfTrueIfFalse(
            as_inlinable_block(&arguments[0])?,
            as_inlinable_block(&arguments[1])?,
        )),
        "ifFalse:ifTrue:" => Some(InlineForm::IfTrueIfFalse(
            as_inlinable_block(&arguments[1])?,
            as_inlinable_block(&arguments[0])?,
        )),
        "ifTrue:" => Some(InlineForm::IfTrue(as_inlinable_block(&arguments[0])?)),
        "ifFalse:" => Some(InlineForm::IfFalse(as_inlinable_block(&arguments[0])?)),
        "and:" => Some(InlineForm::And(as_inlinable_block(&arguments[0])?)),
        "or:" => Some(InlineForm::Or(as_inlinable_block(&arguments[0])?)),
        "whileTrue:" => Some(InlineForm::WhileTrue {
            condition: as_inlinable_block(&send.receiver)?,
            body: as_inlinable_block(&arguments[0])?,
        }),
        "whileFalse:" => Some(InlineForm::WhileFalse {
            condition: as_inlinable_block(&send.receiver)?,
            body: as_inlinable_block(&arguments[0])?,
        }),
        _ => None,
    }
}

/// The arithmetic counterpart to `recognize`: which binary send has an
/// inlined fast path. Pure recognition, like `recognize` above -- the
/// emission is `lower.rs`'s `lower_integer_binary`.
///
/// The gate in front of this is the same `is_ordinary_send`: a `super` or
/// lexical send is never inlined, because its lookup does not start where an
/// explicit-receiver send's does.
pub(crate) fn recognize_integer_binary(send: &ast::Send) -> Option<IntegerOperation> {
    let ast::Selector::Binary(selector) = &send.message.selector else {
        return None;
    };
    match selector.as_str() {
        "+" => Some(IntegerOperation::Add),
        "-" => Some(IntegerOperation::Subtract),
        "*" => Some(IntegerOperation::Multiply),
        "//" => Some(IntegerOperation::FloorDivide),
        "<" => Some(IntegerOperation::Less),
        ">" => Some(IntegerOperation::Greater),
        "<=" => Some(IntegerOperation::LessOrEqual),
        ">=" => Some(IntegerOperation::GreaterOrEqual),
        "=" => Some(IntegerOperation::Equal),
        _ => None,
    }
}

/// A send to the VM mirror whose failure block the optimized lowering lowers
/// inline (`lower.rs`'s `lower_mirror_send`): marked `Binding::MirrorSend` by
/// `resolve_kernel`, its selector a row of `MIRROR` -- which fixes its last
/// keyword as `ifFail:` -- and its last argument a literal block with no
/// parameters and no temporaries, the condition every inlined block meets.
///
/// Asked by the same three places `recognize` is (`lower_send`,
/// `send_writes_register`, `capture::Walker::send`), so the lowering and the
/// capture analysis agree about which block literals are closures. The
/// reference lowering asks it too, to mark the send `SendInlinable`, so that
/// the failure block's activation is exempt from the depth count as the
/// optimized lowering, which never activates it, would have it. That mark
/// covers the whole send, so it would exempt a block the leaf itself
/// activates just as well; the kernel therefore passes a literal block only
/// as the final `ifFail:` argument, which the bootstrap enforces
/// (`interpreter::kernel`'s `check_mirror_send_arguments`).
///
/// `MirrorSend` is never on a `super` or lexical send, so a recognized send
/// is always ordinary; each caller still asks `is_ordinary_send` first, as it
/// does for `recognize`.
pub(crate) fn recognize_mirror_send<'a>(
    bindings: &HashMap<Span, Binding>,
    send: &'a ast::Send,
) -> Option<(MirrorPrimitive, &'a ast::BlockExpression)> {
    if !matches!(bindings.get(&send.span), Some(Binding::MirrorSend)) {
        return None;
    }
    let ast::Selector::Keyword(selector) = &send.message.selector else {
        return None;
    };
    let row = crate::builtins::primitive::mirror_primitive_named(selector)?;
    let block = as_inlinable_block(send.message.arguments.last()?)?;
    Some((row, block))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::formatdoc;

    fn parse_send(source: &str) -> ast::Send {
        let full = formatdoc! {"
            class X = Object new (
            ) (
              foo = ( ^{source} )
            ) : ()
        "};
        let module = ns_syntax::parse(&full).expect("parse");
        let method = module
            .root_class
            .instance_side
            .methods()
            .next()
            .expect("a method");
        match &method.body.statements[0].kind {
            ast::StatementKind::Return(ast::Expression {
                kind: ast::ExpressionKind::Send(send),
                ..
            }) => (**send).clone(),
            other => panic!("expected a return-of-send statement, got {other:?}"),
        }
    }

    /// `source` as the body of a method in a kernel-shaped module, resolved
    /// by `resolve_kernel`; answers the send it returns and the bindings.
    fn kernel_send(source: &str) -> (ast::Send, std::rc::Rc<HashMap<Span, Binding>>) {
        let full = formatdoc! {"
            class K vmMirror: theMirror = Object new (
              | private vmMirror = theMirror. |
            ) (
              class C = Object new () (
                foo: argument = ( ^{source} )
              )
            ) : ()
        "};
        let module = ns_syntax::parse(&full).expect("parse");
        let Ok(resolved) = crate::resolver::resolve_kernel(
            module,
            std::rc::Rc::from(full.as_str()),
            crate::SourceOrigin::File(std::path::PathBuf::from("t.ns")),
        ) else {
            panic!("the kernel-shaped module resolves");
        };
        let nested = resolved
            .module
            .root_class
            .instance_side
            .nested_classes()
            .next()
            .expect("a nested class");
        let send = match &nested
            .instance_side
            .methods()
            .next()
            .expect("a method")
            .body
            .statements[0]
            .kind
        {
            ast::StatementKind::Return(ast::Expression {
                kind: ast::ExpressionKind::Send(send),
                ..
            }) => (**send).clone(),
            other => panic!("expected a return-of-send statement, got {other:?}"),
        };
        (send, resolved.bindings.clone())
    }

    #[test]
    fn recognizes_a_mirror_send_with_a_literal_failure_block() {
        let (send, bindings) = kernel_send("vmMirror stringSize: self ifFail: [^nil]");
        let recognized = recognize_mirror_send(&bindings, &send).map(|(row, _)| row.index());
        assert_eq!(
            recognized,
            crate::builtins::primitive::mirror_primitive_named("stringSize:ifFail:")
                .map(|row| row.index())
        );
        assert!(recognized.is_some());
    }

    #[test]
    fn does_not_recognize_a_mirror_send_whose_failure_argument_is_not_a_literal_block() {
        let (send, bindings) = kernel_send("vmMirror stringSize: self ifFail: argument");
        assert!(recognize_mirror_send(&bindings, &send).is_none());
    }

    #[test]
    fn does_not_recognize_a_selector_the_mirror_does_not_have() {
        let (send, bindings) = kernel_send("vmMirror frobnicate: self ifFail: [^nil]");
        assert!(recognize_mirror_send(&bindings, &send).is_none());
    }

    /// The same text in a module `resolve` resolved is an ordinary send:
    /// only a shipped module's `vmMirror` is known to hold the mirror.
    #[test]
    fn does_not_recognize_a_mirror_send_outside_the_kernel() {
        let send = parse_send("vmMirror stringSize: self ifFail: [^nil]");
        assert!(recognize_mirror_send(&HashMap::new(), &send).is_none());
    }

    #[test]
    fn recognizes_if_true_if_false_with_literal_blocks() {
        assert!(matches!(
            recognize(&parse_send("x ifTrue: [1] ifFalse: [2]")),
            Some(InlineForm::IfTrueIfFalse(..))
        ));
        assert!(matches!(
            recognize(&parse_send("x ifFalse: [1] ifTrue: [2]")),
            Some(InlineForm::IfTrueIfFalse(..))
        ));
        assert!(matches!(
            recognize(&parse_send("x and: [y]")),
            Some(InlineForm::And(_))
        ));
    }

    #[test]
    fn does_not_recognize_a_non_literal_block_argument() {
        assert!(recognize(&parse_send("x ifTrue: y ifFalse: [2]")).is_none());
    }

    #[test]
    fn does_not_recognize_a_block_with_parameters_or_temporaries() {
        assert!(recognize(&parse_send("x ifTrue: [:p | p] ifFalse: [2]")).is_none());
        assert!(recognize(&parse_send("x ifTrue: [ | t | t ] ifFalse: [2]")).is_none());
    }

    #[test]
    fn recognizes_while_only_when_receiver_and_argument_are_both_block_literals() {
        assert!(matches!(
            recognize(&parse_send("[x] whileTrue: [y]")),
            Some(InlineForm::WhileTrue { .. })
        ));
        assert!(matches!(
            recognize(&parse_send("[x] whileFalse: [y]")),
            Some(InlineForm::WhileFalse { .. })
        ));
        assert!(recognize(&parse_send("x whileTrue: [y]")).is_none());
    }

    #[test]
    fn ignores_unary_binary_and_unrelated_keyword_selectors() {
        assert!(recognize(&parse_send("x foo")).is_none());
        assert!(recognize(&parse_send("x + [1]")).is_none());
        assert!(recognize(&parse_send("x at: [1]")).is_none());
    }

    #[test]
    fn every_binary_integer_primitive_is_recognized() {
        // All nine binary members of `IntegerPrimitive`. If a tenth is ever
        // added, this test is where its absence shows.
        let cases = [
            ("1 + 2", IntegerOperation::Add),
            ("1 - 2", IntegerOperation::Subtract),
            ("1 * 2", IntegerOperation::Multiply),
            ("1 // 2", IntegerOperation::FloorDivide),
            ("1 < 2", IntegerOperation::Less),
            ("1 > 2", IntegerOperation::Greater),
            ("1 <= 2", IntegerOperation::LessOrEqual),
            ("1 >= 2", IntegerOperation::GreaterOrEqual),
            ("1 = 2", IntegerOperation::Equal),
        ];
        for (source, expected) in cases {
            let send = parse_send(source);
            assert_eq!(
                recognize_integer_binary(&send),
                Some(expected),
                "expected `{source}` to be recognized"
            );
        }
    }

    #[test]
    fn a_selector_without_an_integer_primitive_is_not_recognized() {
        // `/` exists only as a float primitive, so an integer receiver has no `/`
        // to reach: inlining it would be a fast path to a lookup failure. The
        // others are not binary sends at all.
        for source in ["1 / 2", "1 max: 2", "1 printString"] {
            let send = parse_send(source);
            assert_eq!(
                recognize_integer_binary(&send),
                None,
                "expected `{source}` not to be recognized"
            );
        }
    }
}
