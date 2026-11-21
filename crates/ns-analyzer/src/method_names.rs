//! The methods of one document named as every tool names them, for the
//! custom request `newspeak/methodNames`: the JetBrains plugin watches a
//! method by the name the trace filter reads
//! (`ns_runtime::tool_names`, `docs/design/developer-tool.md`,
//! decision 23), and parses no Newspeak of its own to make it. And the
//! module's id and version, for `newspeak/moduleIdentity`: the plugin
//! passes a run only the watched patterns that name a module of its
//! program, since `ns-next run` refuses one naming no module it has.

use ns_syntax::Span;
use ns_syntax::ast;

/// The method of the request.
pub const METHOD: &str = "newspeak/methodNames";

/// The method of the request for the module's id and version.
pub const IDENTITY_METHOD: &str = "newspeak/moduleIdentity";

/// The id and version the loader gives a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleIdentity {
    pub id: String,
    pub version: Option<String>,
}

/// The id and version the loader gives `module`, parsed from `text`
/// (`ns_runtime::modules::declared_identity`); `None` where the loader
/// refuses its `:module:`, or its class has no name yet, so that no tool
/// can name it.
pub fn module_identity(module: &ast::Module, text: &str) -> Option<ModuleIdentity> {
    if module.root_class.name.name.is_empty() {
        return None;
    }
    let (id, version) = ns_runtime::modules::declared_identity(module, text).ok()?;
    Some(ModuleIdentity {
        id: id.as_str().to_string(),
        version: version.map(|version| version.to_string()),
    })
}

/// One method: its name in the tools' syntax, the span of its
/// declaration and the span of its pattern, in byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodName {
    pub name: String,
    pub span: Span,
    pub selection: Span,
}

/// Every method of `module` in source order, named
/// `<id>[/<Inner>(.<Inner>)*][ class]>><selector>`. The id is `identity`'s,
/// the one the loader gives the module (`module_identity`), its version
/// left out, since a name without one covers every version. A module
/// without an identity has no name a tool would read, so it answers none.
pub fn method_names(module: &ast::Module, identity: Option<&ModuleIdentity>) -> Vec<MethodName> {
    let Some(identity) = identity else {
        return Vec::new();
    };
    let mut names = Vec::new();
    collect(&module.root_class, &identity.id, &mut names);
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
