//! Compile-time recognition of the standard Smalltalk control-flow
//! "special selectors" for inlining. Pure recognition only -- no bytecode
//! emission lives here (see `lower.rs`'s `lower_if`/`lower_and_or`/
//! `lower_while`). See `docs/design/compiler.md`, "Control-flow inlining".

use ns_syntax::ast;

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
    match expression {
        ast::Expression::Block(block)
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

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::formatdoc;

    fn parse_send(source: &str) -> ast::Send {
        let full = formatdoc! {"
            Newspeak3
            'T'
            class X = Object new (
            ) (
              foo = ( ^{source} )
            ) : ()
        "};
        let module = ns_syntax::parse(&full).expect("parse");
        match &module.root_class.instance_side.methods[0].body.statements[0] {
            ast::Statement::Return(ast::Expression::Send(send)) => (**send).clone(),
            other => panic!("expected a return-of-send statement, got {other:?}"),
        }
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
}
