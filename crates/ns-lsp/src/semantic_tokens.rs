//! What the resolver knows about each name, as LSP semantic tokens.

use std::collections::HashMap;

use ns_runtime::Binding;
use ns_syntax::ast;
use ns_syntax::{Span, Token};

/// The token types of the legend, in legend order (`legend_types`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenCategory {
    Parameter,
    Variable,
    Property,
    Class,
    Method,
}

/// One classified name: where it is, what it is, and whether this is its
/// declaration and whether it can never be assigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Classified {
    pub span: Span,
    pub category: TokenCategory,
    pub declaration: bool,
    pub readonly: bool,
}

/// Every name in `module` the resolver can place. Task 4 implements it.
pub fn classify(
    _module: &ast::Module,
    _bindings: &HashMap<Span, Binding>,
    _tokens: &[Token],
) -> Vec<Classified> {
    Vec::new()
}
