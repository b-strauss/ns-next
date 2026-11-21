//! The methods of one document named as every tool names them, for the
//! custom request `newspeak/methodNames`: the JetBrains plugin watches a
//! method by the name the trace filter reads
//! (`ns_runtime::tool_names`, `docs/design/developer-tool.md`,
//! decision 23), and parses no Newspeak of its own to make it.

use ns_syntax::Span;
use ns_syntax::ast;

/// The method of the request.
pub const METHOD: &str = "newspeak/methodNames";

/// One method: its name in the tools' syntax, the span of its
/// declaration and the span of its pattern, in byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodName {
    pub name: String,
    pub span: Span,
    pub selection: Span,
}

/// Every method of `module`, parsed from `text`, in source order, named
/// `<id>[/<Inner>(.<Inner>)*][ class]>><selector>`. The id is the one the
/// loader gives the module (`ns_runtime::modules::declared_identity`),
/// its version left out, since a name without one covers every version.
/// A module whose `:module:` the loader refuses, or whose class has no
/// name yet, has no name a tool would read, so it answers none.
pub fn method_names(module: &ast::Module, text: &str) -> Vec<MethodName> {
    if module.root_class.name.name.is_empty() {
        return Vec::new();
    }
    let Ok((id, _)) = ns_runtime::modules::declared_identity(module, text) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    collect(&module.root_class, id.as_str(), &mut names);
    // Each class's members are collected side by side, its nested classes'
    // after them; ordering by position puts every method where it stands.
    names.sort_by_key(|name| name.span.start);
    names
}

/// The methods of `declaration` and of every class nested in it, which
/// `class_name` -- the module's id and the path below it -- names.
fn collect(declaration: &ast::ClassDeclaration, class_name: &str, names: &mut Vec<MethodName>) {
    let sides = std::iter::once((&declaration.instance_side, ""))
        .chain(declaration.class_side.iter().map(|side| (side, " class")));
    for (side, suffix) in sides {
        for method in side.methods() {
            let selection = crate::document_symbols::pattern_span(&method.pattern);
            names.push(MethodName {
                name: format!("{class_name}{suffix}>>{}", selector(&method.pattern)),
                span: method.span.merge(selection),
                selection,
            });
        }
    }
    for nested in declaration.instance_side.nested_classes() {
        // A class still being typed has no name to put in a path.
        if nested.name.name.is_empty() {
            continue;
        }
        let separator = if class_name.contains('/') { '.' } else { '/' };
        collect(
            nested,
            &format!("{class_name}{separator}{}", nested.name.name),
            names,
        );
    }
}

/// The selector a pattern declares: its name, its operator, or its
/// keywords each with its colon.
fn selector(pattern: &ast::MessagePattern) -> String {
    match pattern {
        ast::MessagePattern::Unary { selector, .. }
        | ast::MessagePattern::Binary { selector, .. } => selector.name.clone(),
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|part| format!("{}:", part.keyword.name))
            .collect(),
    }
}
