//! The breakable statements of a module no interpreter loaded, for the
//! language server, which tells the IDE before any session runs on which
//! lines a breakpoint can stand and which block bodies begin there
//! (`docs/design/debugger.md`, "A breakpoint names a source position").
//! The module is compiled through the metadata's collector exactly as a
//! loaded module's level is compiled again for `SetBreakpoints`, so that
//! the answer is what a breakpoint set there lands on, and nothing the
//! compile builds is kept.

use ns_syntax::{ast, Span};

use super::again::compile_unloaded_level;
use crate::compiler::Lowering;
use crate::debugger::breakpoints::breakable;
use crate::interpreter::Interpreter;
use crate::types::ResolvedModule;

/// A statement a line breakpoint can stand at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakableStatement {
    /// The statement's span in the module's text: under the optimized
    /// lowering, a `SetBreakpoints` at any offset after the previous
    /// breakable statement's start, up to and including this one's, lands
    /// on it; the reference lowering may land such an offset on a
    /// statement the optimized one compiles to no instruction.
    pub statement: Span,
    /// The block literal whose body this statement is the first breakable
    /// statement of -- a closure's and an inlined block's alike, which
    /// the IDE offers as a variant of the line's breakpoint; `None` for
    /// every other statement.
    pub block: Option<Span>,
}

impl Interpreter {
    /// Every statement of `resolved` a line breakpoint can stand at, as a
    /// `SetBreakpoints` of the loaded module would resolve one: of every
    /// body of every level of the module, both sides, its methods, blocks,
    /// slot initializers, superclass clauses and initializer expressions,
    /// each statement that compiled to an instruction under the optimized
    /// lowering, whatever this interpreter's own. A statement compiled
    /// twice -- an inlined block's, inline and as its fallback closure --
    /// is answered once, and of two starting at one offset only the
    /// outer, the one a breakpoint there lands on. Sorted by start.
    ///
    /// Nothing is loaded and nothing runs: the module's levels are
    /// compiled and dropped, and no mixin, code id, module or class
    /// remains in the interpreter, so that a language server may ask for
    /// every version of a document it holds.
    pub fn breakable_statements(&mut self, resolved: &ResolvedModule) -> Vec<BreakableStatement> {
        let mut spans: Vec<Span> = self.enter(|mutator| {
            // The lowering every program runs under. The reference
            // lowering compiles some statements to an instruction the
            // optimized one compiles to none -- a block's last statement
            // that only names a variable, whose register the block
            // answers -- and the answer, given before any session, cannot
            // depend on the lowering a session may choose; a breakpoint
            // a session under the reference lowering resolves to such a
            // statement is reported moved.
            let lowering = Lowering::Optimized;
            let mut spans = Vec::new();
            for declaration in declarations(&resolved.module.root_class) {
                let bodies = compile_unloaded_level(mutator, resolved, declaration, lowering);
                spans.extend(breakable(&bodies).map(|(_, span, _)| span));
            }
            spans
        });
        // `SetBreakpoints` lands on the first statement starting at or
        // after its offset, and of two starting together on the outer
        // (`breakpoints::first_statement_from`): an inner one at the same
        // start is never landed on.
        spans.sort_by_key(|span| (span.start, std::cmp::Reverse(span.end)));
        spans.dedup_by_key(|span| span.start);
        let blocks = block_spans(&resolved.module.root_class);
        let mut opened = std::collections::HashSet::new();
        spans
            .into_iter()
            .map(|statement| {
                // Statements come by start, so the first one met whose
                // innermost block is a block is that block's first.
                let block =
                    innermost_block(&blocks, statement).filter(|block| opened.insert(*block));
                BreakableStatement { statement, block }
            })
            .collect()
    }
}

/// `root` and every declaration nested in it, at any depth: a worklist,
/// since the nesting a text writes is the parser's to bound.
fn declarations(root: &ast::ClassDeclaration) -> Vec<&ast::ClassDeclaration> {
    let mut found = Vec::new();
    let mut pending = vec![root];
    while let Some(declaration) = pending.pop() {
        pending.extend(declaration.instance_side.nested_classes());
        found.push(declaration);
    }
    found
}

/// The span of every block literal of `root`'s declaration and every
/// declaration nested in it. A worklist rather than a recursion: a chain
/// of messages is as deep as the source is long.
fn block_spans(root: &ast::ClassDeclaration) -> Vec<Span> {
    let mut expressions: Vec<&ast::Expression> = Vec::new();
    for declaration in declarations(root) {
        let clauses = declaration
            .superclass
            .iter()
            .chain(&declaration.mixin_applications);
        for clause in clauses {
            expressions.extend(
                clause
                    .messages
                    .iter()
                    .flat_map(|message| &message.arguments),
            );
        }
        let sides = std::iter::once(&declaration.instance_side).chain(&declaration.class_side);
        for side in sides {
            expressions.extend(side.slots().filter_map(|slot| slot.initializer.as_ref()));
            expressions.extend(
                side.initializer_expressions
                    .iter()
                    .map(statement_expression),
            );
            for method in side.methods() {
                push_body(&mut expressions, &method.body);
            }
        }
    }
    let mut blocks = Vec::new();
    while let Some(expression) = expressions.pop() {
        match &expression.kind {
            ast::ExpressionKind::Identifier(_) | ast::ExpressionKind::Literal(_) => {}
            ast::ExpressionKind::Send(send) => {
                expressions.push(&send.receiver);
                expressions.extend(&send.message.arguments);
            }
            ast::ExpressionKind::Cascade(cascade) => {
                expressions.push(&cascade.receiver);
                let messages = std::iter::once(&cascade.first).chain(&cascade.rest);
                expressions.extend(messages.flat_map(|message| &message.arguments));
            }
            ast::ExpressionKind::Assign { value, .. } => expressions.push(value),
            ast::ExpressionKind::Block(block) => {
                blocks.push(block.span);
                push_body(&mut expressions, &block.body);
            }
            ast::ExpressionKind::DynamicArray { elements, .. } => expressions.extend(elements),
        }
    }
    blocks
}

/// The expressions of `body`: its temporaries' initializers and its
/// statements'.
fn push_body<'module>(
    expressions: &mut Vec<&'module ast::Expression>,
    body: &'module ast::CodeBody,
) {
    expressions.extend(
        body.temporaries
            .iter()
            .filter_map(|temporary| temporary.initializer.as_ref()),
    );
    expressions.extend(body.statements.iter().map(statement_expression));
}

/// The one expression a statement holds, a return's included, so that the
/// walk for block literals sees every statement alike.
fn statement_expression(statement: &ast::Statement) -> &ast::Expression {
    match &statement.kind {
        ast::StatementKind::Expression(expression) | ast::StatementKind::Return(expression) => {
            expression
        }
    }
}

/// The innermost of `blocks` whose body holds `statement`: one that
/// starts before it and ends with it or after. A statement starting where
/// a block starts is that block, an expression of the body around it.
fn innermost_block(blocks: &[Span], statement: Span) -> Option<Span> {
    blocks
        .iter()
        .filter(|block| block.start < statement.start && statement.end <= block.end)
        .max_by_key(|block| block.start)
        .copied()
}
