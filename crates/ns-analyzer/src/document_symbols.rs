//! The symbols of one document, for an editor's structure view and
//! breadcrumbs: each class with its factory, then its instance-side members
//! and then its class-side methods, each group in source order, each
//! method's temporaries. No node stands for a side: a class-side method is
//! told apart by its `detail` (`public class`), as Java tells a static member
//! apart by a mark on its icon, so breadcrumbs and sticky lines go from a
//! class straight to its method. They come from the AST `parse_for_editor`
//! answers, so they survive while a file is typed. See
//! `docs/design/editor-support.md`, "Structure".

use std::collections::{HashMap, HashSet};

use lsp_types::{DocumentSymbol, SymbolKind};
use ns_runtime::Binding;
use ns_syntax::ast::{self, AccessModifier, SlotMutability};
use ns_syntax::Span;

use crate::documents::LineIndex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolCategory {
    Class,
    Factory,
    /// A slot with a getter only.
    ReadOnlySlot,
    /// A slot with a getter and a setter.
    ReadWriteSlot,
    Method,
    /// A method's temporary declared with `=`, which nothing assigns.
    ImmutableTemporary,
    /// A method's temporary declared with `::=`, or bare.
    MutableTemporary,
}

/// One symbol, in byte offsets: `span` encloses it, `selection` is what an
/// editor selects when it is picked, always inside `span`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub category: SymbolCategory,
    /// A slot declared `lazy`; never anything else.
    pub lazy: bool,
    /// A method declared on the class side; never anything else.
    pub class_side: bool,
    /// A class that names `Value` in its superclass or `<:` clauses; never
    /// anything else.
    pub value: bool,
    /// The effective access of a class, factory, slot or method; `None` for
    /// a temporary, which has none.
    pub access: Option<AccessModifier>,
    pub span: Span,
    pub selection: Span,
    pub children: Vec<Symbol>,
}

/// The module's class and everything in it. `bindings` are the resolver's
/// for the same module (`ns_runtime::resolve_for_editor`); they tell which
/// declaration a nested class's superclass or `<:` clause names.
pub fn symbols(module: &ast::Module, bindings: &HashMap<Span, Binding>) -> Vec<Symbol> {
    let value = value_classes(&module.root_class, bindings);
    // §6.3.1: "The access level of a top-level class is always public".
    vec![class(
        &module.root_class,
        Some(AccessModifier::Public),
        &value,
        true,
    )]
}

/// The access a member answers to: what it declares, else protected
/// (§6.3.1: "If no access modifier is declared, the default is protected").
fn effective(declared: Option<AccessModifier>) -> Option<AccessModifier> {
    Some(declared.unwrap_or(AccessModifier::Protected))
}

/// `value` holds the nested classes shown as value classes (`value_classes`);
/// `root` marks the module's root class, whose answer is the root rule's.
fn class(
    declaration: &ast::ClassDeclaration,
    access: Option<AccessModifier>,
    value: &HashSet<Span>,
    root: bool,
) -> Symbol {
    let name = if declaration.name.name.is_empty() {
        "(unnamed)".to_string()
    } else {
        declaration.name.name.clone()
    };
    let mut children = vec![factory(declaration)];
    children.extend(instance_members(&declaration.instance_side, value));
    if let Some(class_side) = &declaration.class_side {
        children.extend(class_side.methods().map(|declaration| Symbol {
            class_side: true,
            ..method(declaration)
        }));
    }
    Symbol {
        name,
        category: SymbolCategory::Class,
        lazy: false,
        class_side: false,
        value: if root {
            ns_runtime::root_class_is_value(declaration)
        } else {
            value.contains(&declaration.name.span)
        },
        access,
        span: declaration.span.merge(declaration.name.span),
        selection: declaration.name.span,
        children,
    }
}

/// The nested classes shown as value classes, by the span of their names.
/// Unlike a root class's, a nested class's clauses are expressions
/// evaluated in the enclosing object at run time (§6.1), so the editor can
/// only approximate: a clause whose receiver the resolver binds to a nested
/// class declared in this module makes the class a value class when that
/// class is one, or is the class named `Value` itself (as `Kernel.ns`
/// declares it); any other receiver -- a slot, a method, a name only
/// inherited, `super` -- counts when it is named `Value`. A class that
/// reaches `Value` through a class from another module, or through a
/// `Value` slot holding something else, is misread.
///
/// Each class's clauses are read once into edges to the classes they name,
/// and value-ness is spread backwards along them from the classes that reach
/// `Value` directly: a polynomial bound in the classes and clauses, no class
/// decided twice, and the same whichever class is asked first, a cycle of
/// clauses included.
fn value_classes(root: &ast::ClassDeclaration, bindings: &HashMap<Span, Binding>) -> HashSet<Span> {
    let mut value = HashSet::new();
    let mut named_by: HashMap<Span, Vec<Span>> = HashMap::new();
    let mut chain = vec![root];
    collect_clauses(root, bindings, &mut chain, &mut value, &mut named_by);
    let mut pending: Vec<Span> = value.iter().copied().collect();
    while let Some(target) = pending.pop() {
        for &naming in named_by.get(&target).into_iter().flatten() {
            if value.insert(naming) {
                pending.push(naming);
            }
        }
    }
    value
}

/// Reads the clauses of every class nested in `chain[0]`, `chain` being the
/// class declaring them and then the classes enclosing it.
fn collect_clauses<'a>(
    declaring: &'a ast::ClassDeclaration,
    bindings: &HashMap<Span, Binding>,
    chain: &mut Vec<&'a ast::ClassDeclaration>,
    value: &mut HashSet<Span>,
    named_by: &mut HashMap<Span, Vec<Span>>,
) {
    for nested in declaring.instance_side.nested_classes() {
        for clause in nested.superclass.iter().chain(&nested.mixin_applications) {
            match clause_target(clause, bindings, chain) {
                Some(target) if target.name.name == "Value" => {
                    value.insert(nested.name.span);
                }
                Some(target) => named_by
                    .entry(target.name.span)
                    .or_default()
                    .push(nested.name.span),
                None if clause.receiver.name == "Value" => {
                    value.insert(nested.name.span);
                }
                None => {}
            }
        }
        chain.insert(0, nested);
        collect_clauses(nested, bindings, chain, value, named_by);
        chain.remove(0);
    }
}

/// The nested class of this module a clause's receiver is bound to, if any.
/// The clause runs in the declaring class (`chain[0]`), so a `LexicalSend`'s
/// depth counts out from there; a nested class is declared on an instance
/// side.
fn clause_target<'a>(
    clause: &ast::SuperclassClause,
    bindings: &HashMap<Span, Binding>,
    chain: &[&'a ast::ClassDeclaration],
) -> Option<&'a ast::ClassDeclaration> {
    let Some(Binding::LexicalSend { depth }) = bindings.get(&clause.receiver.span) else {
        return None;
    };
    let depth = usize::from(*depth);
    chain
        .get(depth)?
        .instance_side
        .nested_classes()
        .find(|nested| nested.name.name == clause.receiver.name)
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
        class_side: false,
        value: false,
        access: Some(AccessModifier::Public),
        span,
        selection: span,
        children: Vec::new(),
    }
}

/// Header slots, lazy slots, nested classes and methods, in source order.
fn instance_members(body: &ast::ClassBody, value: &HashSet<Span>) -> Vec<Symbol> {
    let mut members: Vec<Symbol> = body.slots().map(slot).collect();
    members.extend(
        body.nested_classes()
            .map(|nested| class(nested, effective(nested.access), value, false)),
    );
    members.extend(body.methods().map(method));
    members.sort_by_key(|member| member.span.start);
    members
}

fn method(declaration: &ast::MethodDeclaration) -> Symbol {
    let selection = pattern_span(&declaration.pattern);
    Symbol {
        name: pattern_text(&declaration.pattern),
        category: SymbolCategory::Method,
        lazy: false,
        class_side: false,
        value: false,
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
    let category = match declaration.mutability {
        SlotMutability::Immutable => SymbolCategory::ImmutableTemporary,
        SlotMutability::MutableInitialized | SlotMutability::MutableBare => {
            SymbolCategory::MutableTemporary
        }
    };
    variable(declaration, category)
}

fn variable(declaration: &ast::SlotDeclaration, category: SymbolCategory) -> Symbol {
    Symbol {
        name: declaration.name.name.clone(),
        category,
        lazy: false,
        class_side: false,
        value: false,
        access: None,
        span: declaration.span.merge(declaration.name.span),
        selection: declaration.name.span,
        children: Vec::new(),
    }
}

/// `foo`, `+ other`, `at: index put: value`.
fn pattern_text(pattern: &ast::MessagePattern) -> String {
    match pattern {
        ast::MessagePattern::Unary { selector, .. } => selector.name.clone(),
        ast::MessagePattern::Binary {
            selector,
            parameter,
        } => {
            format!("{} {}", selector.name, parameter.name.name)
        }
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|part| format!("{}: {}", part.keyword.name, part.parameter.name.name))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// From the pattern's first name to its last. A keyword pattern is never
/// empty (recovery leaves one without a name out whole); the fallback keeps
/// the function total.
fn pattern_span(pattern: &ast::MessagePattern) -> Span {
    match pattern {
        ast::MessagePattern::Unary { selector, .. } => selector.span,
        ast::MessagePattern::Binary {
            selector,
            parameter,
        } => selector.span.merge(parameter.name.span),
        ast::MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|part| part.keyword.span.merge(part.parameter.name.span))
            .reduce(Span::merge)
            .unwrap_or(Span::new(0, 0)),
    }
}

/// The access word, then ` class` for a class-side method, ` value` for a
/// value class and ` lazy` for a lazy slot (`public class`, `public value`,
/// `protected lazy`); `None` where a symbol has none of them. `class` is how
/// a client tells a class-side method from an instance-side one, since no
/// node stands for a side.
fn detail(symbol: &Symbol) -> Option<String> {
    let access = symbol.access.map(|access| match access {
        AccessModifier::Public => "public",
        AccessModifier::Protected => "protected",
        AccessModifier::Private => "private",
    });
    let words: Vec<&str> = access
        .into_iter()
        .chain(symbol.class_side.then_some("class"))
        .chain(symbol.value.then_some("value"))
        .chain(symbol.lazy.then_some("lazy"))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// The LSP form, positions in UTF-16 units. LSP has no read-only property
/// kind; a constant and a property are the nearest to a getter alone and to
/// a getter with a setter. A temporary is a constant when declared with `=`
/// and a variable otherwise; it has no `detail`, which is how a client tells
/// an immutable temporary from a slot with a getter only.
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
                    SymbolCategory::ReadOnlySlot => SymbolKind::CONSTANT,
                    SymbolCategory::ReadWriteSlot => SymbolKind::PROPERTY,
                    SymbolCategory::Method => SymbolKind::METHOD,
                    SymbolCategory::ImmutableTemporary => SymbolKind::CONSTANT,
                    SymbolCategory::MutableTemporary => SymbolKind::VARIABLE,
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
    use indoc::{formatdoc, indoc};

    /// The symbols of a module with the resolver's bindings for it, as the
    /// server computes them.
    fn symbols_of(module: &ast::Module) -> Vec<Symbol> {
        let resolution = ns_runtime::resolve_for_editor(module.clone());
        symbols(&resolution.module, &resolution.bindings)
    }

    fn outline(text: &str) -> String {
        fn render(symbol: &Symbol, depth: usize, lines: &mut Vec<String>) {
            lines.push(format!(
                "{}{:?} {}{}{}",
                "  ".repeat(depth),
                symbol.category,
                symbol.name,
                if symbol.lazy { " (lazy)" } else { "" },
                if symbol.class_side { " (class)" } else { "" }
            ));
            for child in &symbol.children {
                render(child, depth + 1, lines);
            }
        }
        let parsed = ns_syntax::parse_for_editor(text);
        let mut lines = Vec::new();
        for symbol in symbols_of(&parsed.module) {
            render(&symbol, 0, &mut lines);
        }
        lines.join("\n")
    }

    #[test]
    fn a_class_shows_its_factory_then_its_instance_members_then_its_class_side_methods() {
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
                public default = ( | made | ^self make )
            )
        "};
        assert_eq!(
            outline(text),
            indoc! {"
                Class Module
                  Factory usingPlatform: platform
                  ReadWriteSlot first
                  ReadWriteSlot second
                  Class Inner
                    Factory new
                    Method run
                  Method at: index put: value
                    MutableTemporary old
                  ReadOnlySlot cache (lazy)
                  Method + other
                  Method make (class)
                  Method default (class)
                    MutableTemporary made"}
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
                  Method broken
                  Method second
                  Method make (class)"}
        );
    }

    #[test]
    fn an_empty_file_answers_an_unnamed_class() {
        assert_eq!(outline(""), "Class (unnamed)\n  Factory new");
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
        assert_selections_inside(&symbols_of(&ns_syntax::parse_for_editor(text).module));
    }

    #[test]
    fn a_symbol_is_encoded_in_utf16_on_crlf_lines() {
        let text = "class Module = () (\r\n    (* Grüße 😀 *) run = ( ^1 )\r\n)\r\n";
        let parsed = ns_syntax::parse_for_editor(text);
        let line_index = crate::documents::LineIndex::new(text);
        let encoded = encode(&symbols_of(&parsed.module), text, &line_index);
        let method = &encoded[0].children.as_ref().expect("the class's children")[1];
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
        let tree = symbols_of(&parsed.module);
        let members = &tree[0].children[1..];
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
        let slots = &encoded[0].children.as_ref().expect("children")[1..];
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
    fn a_temporary_shows_whether_it_is_immutable() {
        let text = indoc! {"
            class Module = () (
                run = (
                    | a = 1. b ::= 2. c |
                    ^a
                )
            )
        "};
        let parsed = ns_syntax::parse_for_editor(text);
        let tree = symbols_of(&parsed.module);
        let temporaries = &tree[0].children[1].children;
        let shape: Vec<(&str, SymbolCategory)> = temporaries
            .iter()
            .map(|temporary| (temporary.name.as_str(), temporary.category))
            .collect();
        assert_eq!(
            shape,
            [
                ("a", SymbolCategory::ImmutableTemporary),
                ("b", SymbolCategory::MutableTemporary),
                ("c", SymbolCategory::MutableTemporary),
            ]
        );
        let line_index = LineIndex::new(text);
        let encoded = encode(&tree, text, &line_index);
        let method = &encoded[0].children.as_ref().expect("children")[1];
        let kinds: Vec<(SymbolKind, Option<&str>)> = method
            .children
            .as_ref()
            .expect("temporaries")
            .iter()
            .map(|temporary| (temporary.kind, temporary.detail.as_deref()))
            .collect();
        assert_eq!(
            kinds,
            [
                (SymbolKind::CONSTANT, None),
                (SymbolKind::VARIABLE, None),
                (SymbolKind::VARIABLE, None),
            ]
        );
    }

    /// The details of a module's classes, by name, the root first, then
    /// every nested class depth first in source order.
    fn class_details(text: &str) -> Vec<(String, Option<String>)> {
        fn collect(symbol: &DocumentSymbol, found: &mut Vec<(String, Option<String>)>) {
            if symbol.kind == SymbolKind::CLASS {
                found.push((symbol.name.clone(), symbol.detail.clone()));
            }
            for child in symbol.children.iter().flatten() {
                collect(child, found);
            }
        }
        let parsed = ns_syntax::parse_for_editor(text);
        let line_index = LineIndex::new(text);
        let encoded = encode(&symbols_of(&parsed.module), text, &line_index);
        let mut found = Vec::new();
        collect(&encoded[0], &mut found);
        found
    }

    fn named(found: &[(&str, &str)]) -> Vec<(String, Option<String>)> {
        found
            .iter()
            .map(|(name, detail)| (name.to_string(), Some(detail.to_string())))
            .collect()
    }

    /// A root class is a value class exactly when the root rule builds it
    /// over `Value` (`ns_runtime::root_class_is_value`).
    #[test]
    fn a_root_class_is_a_value_class_as_the_root_rule_builds_it() {
        for (header, detail) in [
            ("class Module = Object <: Value ()", "public value"),
            ("class Module = Value new ()", "public value"),
            ("class Module = Object new ()", "public"),
            ("class Module = ()", "public"),
            // A defect: refused, so never built over `Value`.
            ("class Module = Value <: Value ()", "public"),
        ] {
            let text = format!(
                "{header} ()
"
            );
            assert_eq!(
                class_details(&text),
                named(&[("Module", detail)]),
                "{header}"
            );
        }
    }

    /// A nested class whose clause names `Value`, or a nested class of this
    /// module that is a value class, is one; through a sibling declared later
    /// and through an enclosing class's member too.
    #[test]
    fn a_nested_class_is_a_value_class_through_the_class_its_clause_names() {
        let text = indoc! {"
            class Module = (
            ) (
                class Derived = Point ()
                public class Point = Value new () (
                    class Inner = Derived ()
                )
                class Plain = () ()
                class Mixed = Object <: Plain <: Value ()
                class Named = Plain ()
                class First = Second ()
                class Second = First ()
            )
        "};
        assert_eq!(
            class_details(text),
            named(&[
                ("Module", "public"),
                ("Derived", "protected value"),
                ("Point", "public value"),
                ("Inner", "protected value"),
                ("Plain", "protected"),
                ("Mixed", "protected value"),
                ("Named", "protected"),
                ("First", "protected"),
                ("Second", "protected"),
            ])
        );
    }

    /// Whichever of two classes naming each other is reached first, both
    /// are value classes when one of them applies `Value`.
    #[test]
    fn a_cycle_of_clauses_answers_the_same_in_either_order() {
        for (first, second) in [("A", "B"), ("B", "A")] {
            let text = formatdoc! {"
                class Module = () (
                    class {first} = {second} ()
                    class {second} = {first} <: Value ()
                )
            "};
            let found = class_details(&text);
            assert_eq!(
                found,
                named(&[
                    ("Module", "public"),
                    (first, "protected value"),
                    (second, "protected value"),
                ]),
                "{first} first"
            );
        }
    }

    /// Each class's verdict is decided once: a chain of forty classes, each
    /// naming the next twice, would take 2^40 steps if every clause decided
    /// its target afresh.
    #[test]
    fn a_long_chain_of_clauses_is_decided_quickly() {
        let length = 40;
        let mut classes = String::new();
        for index in 0..length {
            let next = index + 1;
            classes.push_str(&format!(
                "    class C{index} = C{next} <: C{next} ()
"
            ));
        }
        classes.push_str(&format!(
            "    class C{length} = Object <: Value ()
"
        ));
        let text = formatdoc! {"
            class Module = () (
            {classes})
        "};
        let started = std::time::Instant::now();
        let found = class_details(&text);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(found.len(), length + 2);
        assert!(found[1..]
            .iter()
            .all(|(_, detail)| detail.as_deref() == Some("protected value")));
        let unvalued = text.replace("Object <: Value", "Object");
        let started = std::time::Instant::now();
        let found = class_details(&unvalued);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(found
            .iter()
            .all(|(_, detail)| !detail.as_deref().unwrap_or("").contains("value")));
    }

    /// A class declared as `Value`, as `Kernel.ns` declares it, makes the
    /// classes over it value classes; a `Value` slot counts by its name, since
    /// what it holds is decided at run time.
    #[test]
    fn a_receiver_named_value_counts_unless_it_names_a_class_of_this_module() {
        let declared = indoc! {"
            class Module = Object <: Value (
            ) (
                public class Value = Object new () ()
                public class Number = Value new () ()
            )
        "};
        assert_eq!(
            class_details(declared),
            named(&[
                ("Module", "public value"),
                ("Value", "public"),
                ("Number", "public value"),
            ])
        );
        let slot = indoc! {"
            class Module usingPlatform: platform = (
                | Value = platform Value. |
            ) (
                class Point = Value ()
            )
        "};
        assert_eq!(
            class_details(slot),
            named(&[("Module", "public"), ("Point", "protected value")])
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
            ) : (
                make = ( ^self new )
                public default = ( ^self new )
            )
        "};
        let parsed = ns_syntax::parse_for_editor(text);
        let line_index = LineIndex::new(text);
        let encoded = encode(&symbols_of(&parsed.module), text, &line_index);
        let details = |symbol: &DocumentSymbol| symbol.detail.clone();
        let root = &encoded[0];
        assert_eq!(details(root).as_deref(), Some("public"));
        let children = root.children.as_ref().expect("children");
        let found: Vec<(&str, Option<String>)> = children
            .iter()
            .map(|member| (member.name.as_str(), details(member)))
            .collect();
        assert_eq!(
            found,
            [
                ("new", Some("public".to_string())),
                ("held", Some("protected".to_string())),
                ("Inner", Some("public".to_string())),
                ("hidden", Some("private".to_string())),
                ("make", Some("protected class".to_string())),
                ("default", Some("public class".to_string())),
            ]
        );
        let temporary = &children[3].children.as_ref().expect("temporaries")[0];
        assert_eq!(temporary.name, "scratch");
        assert_eq!(details(temporary), None);
    }

    /// The JetBrains plugin puts a play button beside `main:args:` when the
    /// root's factory is `new` -- written or omitted -- and `main:args:` is a
    /// public instance-side method (`editors/jetbrains/.../run/NewspeakModuleRun.kt`,
    /// `runnableMain`); these are the names, kinds and details it reads -- a
    /// method named by its pattern, whose selector the plugin takes apart.
    #[test]
    fn a_runnable_root_shows_its_factory_new_and_a_public_instance_side_main() {
        let members = |text: &str| -> Vec<(String, SymbolKind, Option<String>)> {
            let parsed = ns_syntax::parse_for_editor(text);
            let line_index = LineIndex::new(text);
            let encoded = encode(&symbols_of(&parsed.module), text, &line_index);
            encoded[0]
                .children
                .as_ref()
                .expect("children")
                .iter()
                .map(|member| (member.name.clone(), member.kind, member.detail.clone()))
                .collect()
        };
        let omitted = indoc! {"
            class Hello = () (
                public main: platform args: arguments = ( ^0 )
            ) : (
                public main: platform args: arguments = ( ^0 )
            )
        "};
        let expected = vec![
            (
                "new".to_string(),
                SymbolKind::CONSTRUCTOR,
                Some("public".to_string()),
            ),
            (
                "main: platform args: arguments".to_string(),
                SymbolKind::METHOD,
                Some("public".to_string()),
            ),
            (
                "main: platform args: arguments".to_string(),
                SymbolKind::METHOD,
                Some("public class".to_string()),
            ),
        ];
        assert_eq!(members(omitted), expected);
        let written = omitted.replacen("class Hello =", "class Hello new =", 1);
        assert_eq!(members(&written), expected);
        let protected = indoc! {"
            class Hello = () (
                main: platform args: arguments = ( ^0 )
            )
        "};
        assert_eq!(
            members(protected)[1],
            (
                "main: platform args: arguments".to_string(),
                SymbolKind::METHOD,
                Some("protected".to_string()),
            )
        );
    }
}
