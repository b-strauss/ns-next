//! The server's analysis over every Newspeak module the repository holds:
//! the resolver entry must agree with `resolve`, the editor parse with
//! `parse`, and nothing -- not even a module cut off mid-way, which is what
//! a file looks like while it is typed -- may produce an internal error or
//! a recovered declaration with an empty name.

use std::path::{Path, PathBuf};

use ns_syntax::ast;

fn modules() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    collect(&root.join("corpus/modules"), &mut files);
    collect(&root.join("goldens"), &mut files);
    files.push(root.join("crates/ns-runtime/core-library/Kernel.ns"));
    files.sort();
    assert!(files.len() > 200, "found only {} modules", files.len());
    files
}

fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("reading {}: {error}", directory.display()));
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "ns") {
            files.push(path);
        }
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn the_editor_entry_agrees_with_resolve() {
    for path in modules() {
        let text = read(&path);
        let Ok(module) = ns_syntax::parse(&text) else {
            continue;
        };
        let resolution = ns_runtime::resolve_for_editor(module.clone());
        let origin = ns_runtime::SourceOrigin::File(path.clone());
        match ns_runtime::resolve(module, text.as_str().into(), origin) {
            Ok(resolved) => {
                assert!(
                    resolution.errors.is_empty(),
                    "{}: resolve found no error, the editor entry did",
                    path.display()
                );
                assert!(
                    *resolved.bindings == resolution.bindings,
                    "{}: the bindings differ",
                    path.display()
                );
            }
            Err(errors) => {
                let expected: Vec<_> = errors
                    .iter()
                    .map(|error| (error.span, error.message.clone()))
                    .collect();
                let actual: Vec<_> = resolution
                    .errors
                    .iter()
                    .map(|error| (error.span, error.message.clone()))
                    .collect();
                assert_eq!(expected, actual, "{}", path.display());
            }
        }
    }
}

fn assert_no_internal_error(text: &str, context: &str) {
    for finding in ns_analyzer::analysis::analyze(text).findings {
        assert!(
            !finding.message.contains("internal error"),
            "{context}: {}",
            finding.message
        );
    }
}

#[test]
fn no_module_produces_an_internal_error() {
    for path in modules() {
        assert_no_internal_error(&read(&path), &path.display().to_string());
    }
}

#[test]
fn the_editor_parse_agrees_with_parse() {
    for path in modules() {
        let text = read(&path);
        let Ok(module) = ns_syntax::parse(&text) else {
            continue;
        };
        let parsed = ns_syntax::parse_for_editor(&text);
        assert!(parsed.diagnostics.is_empty(), "{}", path.display());
        assert!(
            parsed.module == module,
            "{}: the modules differ",
            path.display()
        );
    }
}

/// The cut points of a module, as `no_truncated_module_produces_an_internal_error`
/// documents: every token end for the goldens and the kernel, 64 spread
/// evenly for the corpus modules. With `mid_token`, the middle of each
/// token too -- a string, comment or keyword half typed.
fn cut_points(path: &Path, text: &str, mid_token: bool) -> Vec<usize> {
    let (tokens, _) = ns_syntax::lexer::Lexer::new(text).run();
    let in_corpus = path
        .components()
        .any(|component| component.as_os_str() == "corpus");
    let stride = if in_corpus {
        (tokens.len() / 64).max(1)
    } else {
        1
    };
    let mut cuts = Vec::new();
    for token in tokens.iter().step_by(stride) {
        cuts.push(token.span.end as usize);
        if mid_token {
            let middle = ((token.span.start + token.span.end) / 2) as usize;
            if text.is_char_boundary(middle) {
                cuts.push(middle);
            }
        }
    }
    cuts
}

/// Every prefix of a module ending at a token boundary or in the middle of
/// a token, for the goldens and the kernel; for the corpus modules, which
/// are far larger, 64 tokens spread evenly over the file. Every prefix of
/// every corpus module is quadratic in its size and would make this test
/// take minutes in a debug build; 64 cut points per module still cut inside
/// every kind of construct the corpus holds.
#[test]
fn no_truncated_module_produces_an_internal_error() {
    for path in modules() {
        let text = read(&path);
        for end in cut_points(&path, &text, true) {
            let context = format!("{} cut at byte {end}", path.display());
            assert_no_internal_error(&text[..end], &context);
        }
    }
}

/// Recovery leaves a declaration without a name out; only a root class
/// keeps an empty one.
#[test]
fn no_truncated_module_declares_an_empty_name() {
    for path in modules() {
        let text = read(&path);
        for end in cut_points(&path, &text, false) {
            let module = ns_syntax::parse_for_editor(&text[..end]).module;
            let mut names = Vec::new();
            declared_names_of_class(&module.root_class, &mut names);
            names.remove(0); // the root class's own name
            assert!(
                names.iter().all(|name| !name.is_empty()),
                "{} cut at byte {end}",
                path.display()
            );
        }
    }
}

/// Every symbol's selection lies inside its range, on every module and every
/// cut of one: a client refuses the whole answer otherwise.
#[test]
fn every_symbol_selection_lies_inside_its_range() {
    fn check(symbols: &[ns_analyzer::document_symbols::Symbol], context: &str) {
        for symbol in symbols {
            assert!(
                symbol.span.start <= symbol.selection.start
                    && symbol.selection.end <= symbol.span.end,
                "{context}: {} {:?} outside {:?}",
                symbol.name,
                symbol.selection,
                symbol.span
            );
            check(&symbol.children, context);
        }
    }
    for path in modules() {
        let text = read(&path);
        for end in cut_points(&path, &text, false) {
            let module = ns_syntax::parse_for_editor(&text[..end]).module;
            let context = format!("{} cut at byte {end}", path.display());
            let resolution = ns_runtime::resolve_for_editor(module);
            check(
                &ns_analyzer::document_symbols::symbols(&resolution.module, &resolution.bindings),
                &context,
            );
        }
    }
}

fn declared_names_of_class(class: &ast::ClassDeclaration, names: &mut Vec<String>) {
    names.push(class.name.name.clone());
    if let Some(factory) = &class.factory {
        declared_names_of_pattern(factory, names);
    }
    for clause in class.superclass.iter().chain(&class.mixin_applications) {
        names.push(clause.receiver.name.clone());
        if let Some(ast::SuperclassPrefix::Outer(name)) = &clause.prefix {
            names.push(name.name.clone());
        }
        for message in &clause.messages {
            message
                .arguments
                .iter()
                .for_each(|argument| declared_names_of_expression(argument, names));
        }
    }
    for body in std::iter::once(&class.instance_side).chain(&class.class_side) {
        for slot in body.slots() {
            declared_names_of_slot(slot, names);
        }
        for statement in &body.initializer_expressions {
            declared_names_of_statement(statement, names);
        }
        for method in body.methods() {
            declared_names_of_pattern(&method.pattern, names);
            declared_names_of_body(&method.body, names);
        }
        for nested in body.nested_classes() {
            declared_names_of_class(nested, names);
        }
    }
}

fn declared_names_of_pattern(pattern: &ast::MessagePattern, names: &mut Vec<String>) {
    match pattern {
        ast::MessagePattern::Unary { selector } => names.push(selector.name.clone()),
        ast::MessagePattern::Binary {
            selector,
            parameter,
        } => {
            names.push(selector.name.clone());
            names.push(parameter.name.clone());
        }
        ast::MessagePattern::Keyword { parts } => {
            for (keyword, parameter) in parts {
                names.push(keyword.name.clone());
                names.push(parameter.name.clone());
            }
        }
    }
}

fn declared_names_of_slot(slot: &ast::SlotDeclaration, names: &mut Vec<String>) {
    names.push(slot.name.name.clone());
    if let Some(initializer) = &slot.initializer {
        declared_names_of_expression(initializer, names);
    }
}

fn declared_names_of_body(body: &ast::CodeBody, names: &mut Vec<String>) {
    for temporary in &body.temporaries {
        declared_names_of_slot(temporary, names);
    }
    for statement in &body.statements {
        declared_names_of_statement(statement, names);
    }
}

fn declared_names_of_statement(statement: &ast::Statement, names: &mut Vec<String>) {
    match &statement.kind {
        ast::StatementKind::Expression(expression) | ast::StatementKind::Return(expression) => {
            declared_names_of_expression(expression, names);
        }
    }
}

fn declared_names_of_expression(expression: &ast::Expression, names: &mut Vec<String>) {
    match &expression.kind {
        ast::ExpressionKind::Identifier(_) | ast::ExpressionKind::Literal(_) => {}
        ast::ExpressionKind::Send(send) => {
            declared_names_of_expression(&send.receiver, names);
            send.message
                .arguments
                .iter()
                .for_each(|argument| declared_names_of_expression(argument, names));
        }
        ast::ExpressionKind::Cascade(cascade) => {
            declared_names_of_expression(&cascade.receiver, names);
            for message in std::iter::once(&cascade.first).chain(&cascade.rest) {
                message
                    .arguments
                    .iter()
                    .for_each(|argument| declared_names_of_expression(argument, names));
            }
        }
        ast::ExpressionKind::Assign { value, .. } => declared_names_of_expression(value, names),
        ast::ExpressionKind::Block(block) => {
            names.extend(
                block
                    .parameters
                    .iter()
                    .map(|parameter| parameter.name.clone()),
            );
            declared_names_of_body(&block.body, names);
        }
        ast::ExpressionKind::DynamicArray { elements, .. } => {
            elements
                .iter()
                .for_each(|element| declared_names_of_expression(element, names));
        }
    }
}
