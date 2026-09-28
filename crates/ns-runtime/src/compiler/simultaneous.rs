//! §6.3.2's simultaneous slot clause, the one desugaring both lowerings and
//! the capture analysis share (`docs/design/object-model.md`,
//! "Simultaneous slots", "The lowering"): each initializer becomes the body
//! of a block, made into a future and stored; the clause's end reads each
//! slot back and stores it resolved. The resolver scoped each initializer as
//! that block's body (`resolver::Resolver::resolve_simultaneous_initializer`),
//! so the bindings inside it already count the block as a scope.

use ns_syntax::ast;

/// `[initializer]` for a simultaneous declaration that has one. Its span is
/// the declaration's: the capture analysis and the lowering key a block by
/// its span (`capture::BodyKey::Block`), and no block literal can share it,
/// since a declaration's span also covers its name.
pub(crate) fn future_block(declaration: &ast::SlotDeclaration) -> Option<ast::Expression> {
    if !declaration.simultaneous {
        return None;
    }
    let initializer = declaration.initializer.as_ref()?;
    Some(ast::Expression::Block(Box::new(ast::BlockExpression {
        parameters: Vec::new(),
        body: ast::CodeBody {
            temporaries: Vec::new(),
            statements: vec![ast::Statement::Expression(initializer.clone())],
            span: declaration.span,
        },
        span: declaration.span,
    })))
}
