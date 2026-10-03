//! The symbols of one document, for an editor's structure view and
//! breadcrumbs: each class with its factory, its instance side and its
//! class side, each side's members in source order, each method's
//! temporaries. They come from the AST `parse_for_editor` answers, so they
//! survive while a file is typed. See `docs/design/editor-support.md`,
//! "Structure".

use lsp_types::{DocumentSymbol, SymbolKind};
use ns_syntax::ast::{self, AccessModifier, SlotMutability};
use ns_syntax::Span;

use crate::documents::LineIndex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolCategory {
    Class,
    Factory,
    Side,
    /// A slot with a getter only.
    ReadOnlySlot,
    /// A slot with a getter and a setter.
    ReadWriteSlot,
    Method,
    Temporary,
}

/// One symbol, in byte offsets: `span` encloses it, `selection` is what an
/// editor selects when it is picked, always inside `span`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub category: SymbolCategory,
    /// A slot declared `lazy`; never anything else.
    pub lazy: bool,
    /// The effective access of a class, factory, slot or method; `None` for
    /// a side or a temporary, which have none.
    pub access: Option<AccessModifier>,
    pub span: Span,
    pub selection: Span,
    pub children: Vec<Symbol>,
}

/// The module's class and everything in it.
pub fn symbols(module: &ast::Module) -> Vec<Symbol> {
    // §6.3.1: "The access level of a top-level class is always public".
    vec![class(&module.root_class, Some(AccessModifier::Public))]
}

/// The access a member answers to: what it declares, else protected
/// (§6.3.1: "If no access modifier is declared, the default is protected").
fn effective(declared: Option<AccessModifier>) -> Option<AccessModifier> {
    Some(declared.unwrap_or(AccessModifier::Protected))
}

fn class(declaration: &ast::ClassDeclaration, access: Option<AccessModifier>) -> Symbol {
    let name = if declaration.name.name.is_empty() {
        "(unnamed)".to_string()
    } else {
        declaration.name.name.clone()
    };
    let mut children = vec![factory(declaration)];
    children.push(side(
        "instance side",
        &declaration.instance_side,
        instance_members(&declaration.instance_side),
        declaration.name.span,
    ));
    if let Some(class_side) = &declaration.class_side {
        children.push(side(
            "class side",
            class_side,
            class_side.methods.iter().map(method).collect(),
            declaration.name.span,
        ));
    }
    Symbol {
        name,
        category: SymbolCategory::Class,
        lazy: false,
        access,
        span: declaration.span.merge(declaration.name.span),
        selection: declaration.name.span,
        children,
    }
}

/// The primary factory: the pattern as written, or `new`, which a class
/// without one answers (§6), placed at the class's name. It is public
/// whatever §6.3.1's default says: the runtime enters every primary factory
/// as public (`ns_runtime::mixin::factory_entry`), since a protected one
/// would leave no class instantiable from outside.
fn factory(declaration: &ast::ClassDeclaration) -> Symbol {
    let (name, span) = match &declaration.factory {
        Some(pattern) => (pattern_text(pattern), pattern_span(pattern)),
        None => ("new".to_string(), declaration.name.span),
    };
    Symbol {
        name,
        category: SymbolCategory::Factory,
        lazy: false,
        access: Some(AccessModifier::Public),
        span,
        selection: span,
        children: Vec::new(),
    }
}

/// A side from its `(` to its `)`, or to where it ends when recovery found
/// no `)`; selected at its `(`, or at the class's name when it has none.
fn side(name: &str, body: &ast::ClassBody, members: Vec<Symbol>, class_name: Span) -> Symbol {
    let (span, selection) = match body.delimiters {
        Some(delimiters) => {
            let end = delimiters.close.map_or(body.span.end, |close| close.end);
            (
                Span::new(delimiters.open.start, end.max(delimiters.open.end)),
                delimiters.open,
            )
        }
        None => (body.span.merge(class_name), class_name),
    };
    Symbol {
        name: name.to_string(),
        category: SymbolCategory::Side,
        lazy: false,
        access: None,
        span,
        selection,
        children: members,
    }
}

/// Header slots, lazy slots, nested classes and methods, in source order.
fn instance_members(body: &ast::ClassBody) -> Vec<Symbol> {
    let mut members: Vec<Symbol> = body.slots.iter().map(slot).collect();
    members.extend(
        body.nested_classes
            .iter()
            .map(|nested| class(nested, effective(nested.access))),
    );
    members.extend(body.methods.iter().map(method));
    members.sort_by_key(|member| member.span.start);
    members
}

fn method(declaration: &ast::MethodDeclaration) -> Symbol {
    let selection = pattern_span(&declaration.pattern);
    Symbol {
        name: pattern_text(&declaration.pattern),
        category: SymbolCategory::Method,
        lazy: false,
        access: effective(declaration.access),
        span: declaration.span.merge(selection),
        selection,
        children: declaration.body.temporaries.iter().map(temporary).collect(),
    }
}

fn slot(declaration: &ast::SlotDeclaration) -> Symbol {
    let category = match declaration.mutability {
        SlotMutability::Immutable => SymbolCategory::ReadOnlySlot,
        SlotMutability::MutableInitialized | SlotMutability::MutableBare => {
            SymbolCategory::ReadWriteSlot
        }
    };
    Symbol {
        lazy: declaration.lazy,
        access: effective(declaration.access),
        ..variable(declaration, category)
    }
}

fn temporary(declaration: &ast::SlotDeclaration) -> Symbol {
    variable(declaration, SymbolCategory::Temporary)
}

fn variable(declaration: &ast::SlotDeclaration, category: SymbolCategory) -> Symbol {
    Symbol {
        name: declaration.name.name.clone(),
        category,
        lazy: false,
        access: None,
        span: declaration.span.merge(declaration.name.span),
        selection: declaration.name.span,
        children: Vec::new(),
    }
}

/// `foo`, `+ other`, `at: index put: value`.
fn pattern_text(pattern: &ast::MessagePattern) -> String {
    match pattern {
        ast::MessagePattern::Unary { selector } => selector.name.clone(),
        ast::MessagePattern::Binary {
            selector,
            parameter,
        } => {
            format!("{} {}", selector.name, parameter.name)
        }
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|(keyword, parameter)| format!("{}: {}", keyword.name, parameter.name))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// From the pattern's first name to its last. A keyword pattern is never
/// empty (recovery leaves one without a name out whole); the fallback keeps
/// the function total.
fn pattern_span(pattern: &ast::MessagePattern) -> Span {
    match pattern {
        ast::MessagePattern::Unary { selector } => selector.span,
        ast::MessagePattern::Binary {
            selector,
            parameter,
        } => selector.span.merge(parameter.span),
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|(keyword, parameter)| keyword.span.merge(parameter.span))
            .reduce(Span::merge)
            .unwrap_or(Span::new(0, 0)),
    }
}

/// The access word, then ` lazy` for a lazy slot; `None` where a symbol has
/// neither.
fn detail(symbol: &Symbol) -> Option<String> {
    let access = symbol.access.map(|access| match access {
        AccessModifier::Public => "public",
        AccessModifier::Protected => "protected",
        AccessModifier::Private => "private",
    });
    match (access, symbol.lazy) {
        (Some(access), true) => Some(format!("{access} lazy")),
        (Some(access), false) => Some(access.to_string()),
        (None, true) => Some("lazy".to_string()),
        (None, false) => None,
    }
}

/// The LSP form, positions in UTF-16 units. LSP has no read-only property
/// kind; a constant and a property are the nearest to a getter alone and to
/// a getter with a setter.
pub fn encode(symbols: &[Symbol], text: &str, line_index: &LineIndex) -> Vec<DocumentSymbol> {
    symbols
        .iter()
        .map(|symbol| {
            let range = |span: Span| {
                lsp_types::Range::new(
                    line_index.position(text, span.start),
                    line_index.position(text, span.end),
                )
            };
            #[allow(deprecated)] // `deprecated` is a required field, superseded by `tags`
            DocumentSymbol {
                name: symbol.name.clone(),
                detail: detail(symbol),
                kind: match symbol.category {
                    SymbolCategory::Class => SymbolKind::CLASS,
                    SymbolCategory::Factory => SymbolKind::CONSTRUCTOR,
                    SymbolCategory::Side => SymbolKind::NAMESPACE,
                    SymbolCategory::ReadOnlySlot => SymbolKind::CONSTANT,
                    SymbolCategory::ReadWriteSlot => SymbolKind::PROPERTY,
                    SymbolCategory::Method => SymbolKind::METHOD,
                    SymbolCategory::Temporary => SymbolKind::VARIABLE,
                },
                tags: None,
                deprecated: None,
                range: range(symbol.span),
                selection_range: range(symbol.selection),
                children: (!symbol.children.is_empty())
                    .then(|| encode(&symbol.children, text, line_index)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    fn outline(text: &str) -> String {
        fn render(symbol: &Symbol, depth: usize, lines: &mut Vec<String>) {
            lines.push(format!(
                "{}{:?} {}{}",
                "  ".repeat(depth),
                symbol.category,
                symbol.name,
                if symbol.lazy { " (lazy)" } else { "" }
            ));
            for child in &symbol.children {
                render(child, depth + 1, lines);
            }
        }
        let parsed = ns_syntax::parse_for_editor(text);
        let mut lines = Vec::new();
        for symbol in symbols(&parsed.module) {
            render(&symbol, 0, &mut lines);
        }
        lines.join("\n")
    }

    #[test]
    fn a_class_shows_its_factory_then_its_instance_side_then_its_class_side() {
        let text = indoc! {"
            class Module usingPlatform: platform = (
                | first second |
            ) (
                class Inner = () (
                    run = ( ^1 )
                )
                at: index put: value = (
                    | old |
                    ^value
                )
                lazy cache = 3.
                + other = ( ^other )
            ) : (
                make = ( ^self usingPlatform: nil )
            )
        "};
        assert_eq!(
            outline(text),
            indoc! {"
                Class Module
                  Factory usingPlatform: platform
                  Side instance side
                    ReadWriteSlot first
                    ReadWriteSlot second
                    Class Inner
                      Factory new
                      Side instance side
                        Method run
                    Method at: index put: value
                      Temporary old
                    ReadOnlySlot cache (lazy)
                    Method + other
                  Side class side
                    Method make"}
        );
    }

    #[test]
    fn a_half_typed_method_keeps_the_symbols_after_it() {
        let text = indoc! {"
            class Module = () (
                broken = (
                    ^1
                second = ( ^2 )
            ) : (
                make = ( ^self new )
            )
        "};
        assert_eq!(
            outline(text),
            indoc! {"
                Class Module
                  Factory new
                  Side instance side
                    Method broken
                    Method second
                  Side class side
                    Method make"}
        );
    }

    #[test]
    fn an_empty_file_answers_an_unnamed_class() {
        assert_eq!(
            outline(""),
            "Class (unnamed)\n  Factory new\n  Side instance side"
        );
    }

    /// Every selection lies inside its range; a client refuses a symbol
    /// otherwise.
    fn assert_selections_inside(symbols: &[Symbol]) {
        for symbol in symbols {
            assert!(
                symbol.span.start <= symbol.selection.start
                    && symbol.selection.end <= symbol.span.end,
                "{:?} {}: {:?} outside {:?}",
                symbol.category,
                symbol.name,
                symbol.selection,
                symbol.span
            );
            assert_selections_inside(&symbol.children);
        }
    }

    #[test]
    fn selections_lie_inside_their_ranges() {
        let text = indoc! {"
            class Module = () (
                run = ( | first | ^first )
                broken = (
            ) : ()
        "};
        assert_selections_inside(&symbols(&ns_syntax::parse_for_editor(text).module));
    }

    #[test]
    fn a_symbol_is_encoded_in_utf16_on_crlf_lines() {
        let text = "class Module = () (\r\n    (* Grüße 😀 *) run = ( ^1 )\r\n)\r\n";
        let parsed = ns_syntax::parse_for_editor(text);
        let line_index = crate::documents::LineIndex::new(text);
        let encoded = encode(&symbols(&parsed.module), text, &line_index);
        let side = &encoded[0].children.as_ref().expect("the class's children")[1];
        let method = &side.children.as_ref().expect("the side's members")[0];
        let line_start = text.find("    (* Grüße").expect("line 1");
        let column = text[line_start..text.find("run").expect("the method")]
            .encode_utf16()
            .count() as u32;
        assert_eq!(method.name, "run");
        assert_eq!(
            (
                method.selection_range.start.line,
                method.selection_range.start.character
            ),
            (1, column)
        );
        assert_eq!(method.kind, SymbolKind::METHOD);
    }

    #[test]
    fn a_slot_shows_its_accessors_and_whether_it_is_lazy() {
        let text = indoc! {"
            class Module = (
                | a = 1. b ::= 2. c |
            ) (
                run = ( ^1 )
                lazy d = 4.
                lazy e ::= 5.
            )
        "};
        let parsed = ns_syntax::parse_for_editor(text);
        let tree = symbols(&parsed.module);
        let members = &tree[0].children[1].children;
        let shape: Vec<(&str, SymbolCategory, bool)> = members
            .iter()
            .map(|member| (member.name.as_str(), member.category, member.lazy))
            .collect();
        assert_eq!(
            shape,
            [
                ("a", SymbolCategory::ReadOnlySlot, false),
                ("b", SymbolCategory::ReadWriteSlot, false),
                ("c", SymbolCategory::ReadWriteSlot, false),
                ("run", SymbolCategory::Method, false),
                ("d", SymbolCategory::ReadOnlySlot, true),
                ("e", SymbolCategory::ReadWriteSlot, true),
            ]
        );
        let line_index = LineIndex::new(text);
        let encoded = encode(&tree, text, &line_index);
        let slots = encoded[0].children.as_ref().expect("children")[1]
            .children
            .as_ref()
            .expect("members");
        let kinds: Vec<(SymbolKind, Option<&str>)> = slots
            .iter()
            .map(|slot| (slot.kind, slot.detail.as_deref()))
            .collect();
        assert_eq!(
            kinds,
            [
                (SymbolKind::CONSTANT, Some("protected")),
                (SymbolKind::PROPERTY, Some("protected")),
                (SymbolKind::PROPERTY, Some("protected")),
                (SymbolKind::METHOD, Some("protected")),
                (SymbolKind::CONSTANT, Some("protected lazy")),
                (SymbolKind::PROPERTY, Some("protected lazy")),
            ]
        );
    }

    #[test]
    fn a_member_shows_its_effective_access() {
        let text = indoc! {"
            class Module = (
                | held |
            ) (
                public class Inner = () ()
                private hidden = ( | scratch | ^1 )
            )
        "};
        let parsed = ns_syntax::parse_for_editor(text);
        let line_index = LineIndex::new(text);
        let encoded = encode(&symbols(&parsed.module), text, &line_index);
        let details = |symbol: &DocumentSymbol| symbol.detail.clone();
        let root = &encoded[0];
        assert_eq!(details(root).as_deref(), Some("public"));
        let children = root.children.as_ref().expect("children");
        assert_eq!(details(&children[0]).as_deref(), Some("public"), "factory");
        assert_eq!(details(&children[1]), None, "side");
        let members = children[1].children.as_ref().expect("members");
        let found: Vec<(&str, Option<String>)> = members
            .iter()
            .map(|member| (member.name.as_str(), details(member)))
            .collect();
        assert_eq!(
            found,
            [
                ("held", Some("protected".to_string())),
                ("Inner", Some("public".to_string())),
                ("hidden", Some("private".to_string())),
            ]
        );
        let temporary = &members[2].children.as_ref().expect("temporaries")[0];
        assert_eq!(temporary.name, "scratch");
        assert_eq!(details(temporary), None);
    }
}
