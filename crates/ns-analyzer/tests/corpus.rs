//! The server's analysis over every Newspeak module the repository holds:
//! the resolver entry must agree with `resolve`, the editor parse with
//! `parse`, and nothing -- not even a module cut off mid-way, which is what
//! a file looks like while it is typed -- may produce an internal error or
//! a recovered declaration with an empty name.
//!
//! The checks over every cut take most of a minute -- the kernel, cut at
//! every token, most of that -- and as one test each they ran alone at the
//! end of every suite while the other cores idled. So each is split into
//! trials (`libtest-mimic`, so `harness = false`) that run in parallel: a
//! module cut at every token and longer than a window, one trial per window
//! of its bytes; the other modules in groups, since every trial is a
//! process of its own and a module's cuts cost less than starting one. The
//! checks of whole modules are cheap and stay one trial each.

use std::path::{Path, PathBuf};

use libtest_mimic::{Arguments, Trial};
use ns_syntax::ast;

/// One check of one module; it panics on a violation, as an assertion does.
type ModuleCheck = fn(&Path);

/// One check of a module's text cut at byte `end`.
type CutCheck = fn(&Path, &str, usize);

/// The checks of a whole module, by name.
const MODULE_CHECKS: [(&str, ModuleCheck); 3] = [
    (
        "the_editor_entry_agrees_with_resolve",
        the_editor_entry_agrees_with_resolve,
    ),
    (
        "no_module_produces_an_internal_error",
        no_module_produces_an_internal_error,
    ),
    (
        "the_editor_parse_agrees_with_parse",
        the_editor_parse_agrees_with_parse,
    ),
];

/// The checks of every cut of a module, by name, each with whether its cuts
/// include the middle of each token.
const CUT_CHECKS: [(&str, bool, CutCheck); 3] = [
    (
        "no_truncated_module_produces_an_internal_error",
        true,
        no_internal_error_at,
    ),
    (
        "no_truncated_module_declares_an_empty_name",
        false,
        no_empty_name_at,
    ),
    (
        "every_symbol_selection_lies_inside_its_range",
        false,
        every_selection_inside_at,
    ),
];

/// The bytes one trial's cuts end in, for a module cut at every token. A
/// cut costs about the length of its prefix, so the windows late in the
/// kernel cost the most; at this size the costliest takes a few seconds in
/// a debug build, below the longest test the suite cannot split.
const WINDOW: usize = 4096;

/// How many of the other modules one trial checks.
const GROUP: usize = 24;

fn main() {
    let arguments = Arguments::from_args();
    let root = repository_root();
    let mut trials = Vec::new();
    // A check fails by panicking, as an assertion does; the trial reports
    // the panic as its failure, and the panic names the module.
    let modules = modules();
    for (check_name, check) in MODULE_CHECKS {
        let modules = modules.clone();
        trials.push(Trial::test(check_name, move || {
            modules.iter().for_each(|path| check(path));
            Ok(())
        }));
    }
    // Only a file's length is read here: every process nextest starts
    // builds this list, so the cuts are found inside each trial.
    let (windowed, grouped): (Vec<PathBuf>, Vec<PathBuf>) = modules.into_iter().partition(|path| {
        !in_corpus(path)
            && std::fs::metadata(path)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
                .len() as usize
                > WINDOW
    });
    for (check_name, mid_token, check) in CUT_CHECKS {
        for path in &windowed {
            let length = std::fs::metadata(path).expect("read above").len() as usize;
            for start in (0..=length).step_by(WINDOW) {
                let window = start..start + WINDOW;
                let name = relative_name(&root, path);
                let trial_name = format!("{check_name}::{name}::bytes_{start}_{}", window.end);
                let path = path.clone();
                trials.push(Trial::test(trial_name, move || {
                    let text = read(&path);
                    for end in cut_points(&path, &text, mid_token) {
                        if window.contains(&end) {
                            check(&path, &text, end);
                        }
                    }
                    Ok(())
                }));
            }
        }
        let groups = grouped.len().div_ceil(GROUP);
        for (index, group) in grouped.chunks(GROUP).enumerate() {
            let group = group.to_vec();
            trials.push(Trial::test(
                format!("{check_name}::modules_{}_of_{groups}", index + 1),
                move || {
                    for path in &group {
                        let text = read(path);
                        for end in cut_points(path, &text, mid_token) {
                            check(path, &text, end);
                        }
                    }
                    Ok(())
                },
            ));
        }
    }
    libtest_mimic::run(&arguments, trials).exit();
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `file` relative to `root`, its directories separated by `/` on every
/// platform.
fn relative_name(root: &Path, file: &Path) -> String {
    let relative = file.strip_prefix(root).unwrap_or(file);
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn modules() -> Vec<PathBuf> {
    let root = repository_root();
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

fn the_editor_entry_agrees_with_resolve(path: &Path) {
    let text = read(path);
    let Ok(module) = ns_syntax::parse(&text) else {
        return;
    };
    let resolution = ns_runtime::resolve_for_editor(module.clone());
    let origin = ns_runtime::SourceOrigin::File(path.to_path_buf());
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

fn assert_no_internal_error(text: &str, context: &str) {
    for finding in ns_analyzer::analysis::analyze(text).findings {
        assert!(
            !finding.message.contains("internal error"),
            "{context}: {}",
            finding.message
        );
    }
}

fn no_module_produces_an_internal_error(path: &Path) {
    assert_no_internal_error(&read(path), &path.display().to_string());
}

fn the_editor_parse_agrees_with_parse(path: &Path) {
    let text = read(path);
    let Ok(module) = ns_syntax::parse(&text) else {
        return;
    };
    let parsed = ns_syntax::parse_for_editor(&text);
    assert!(parsed.diagnostics.is_empty(), "{}", path.display());
    assert!(
        parsed.module == module,
        "{}: the modules differ",
        path.display()
    );
}

/// Whether `path` is a corpus module, cut at 64 points rather than at every
/// token.
fn in_corpus(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "corpus")
}

/// The cut points of a module: every token end for the goldens and the
/// kernel, 64 spread evenly for the corpus modules, which are far larger --
/// every prefix of every corpus module is quadratic in its size and would
/// take minutes in a debug build, and 64 cut points per module still cut
/// inside every kind of construct the corpus holds. With `mid_token`, the
/// middle of each token too: a string, comment or keyword half typed.
fn cut_points(path: &Path, text: &str, mid_token: bool) -> Vec<usize> {
    let (tokens, _) = ns_syntax::lexer::Lexer::new(text).run();
    let stride = if in_corpus(path) {
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

/// A module cut off at a token boundary or in the middle of a token
/// produces no internal error.
fn no_internal_error_at(path: &Path, text: &str, end: usize) {
    let context = format!("{} cut at byte {end}", path.display());
    assert_no_internal_error(&text[..end], &context);
}

/// Recovery leaves a declaration without a name out; only a root class
/// keeps an empty one.
fn no_empty_name_at(path: &Path, text: &str, end: usize) {
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

/// Every symbol's selection lies inside its range, on every cut of a
/// module: a client refuses the whole answer otherwise.
fn every_selection_inside_at(path: &Path, text: &str, end: usize) {
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
    let module = ns_syntax::parse_for_editor(&text[..end]).module;
    let context = format!("{} cut at byte {end}", path.display());
    let resolution = ns_runtime::resolve_for_editor(module);
    check(
        &ns_analyzer::document_symbols::symbols(&resolution.module, &resolution.bindings),
        &context,
    );
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
        ast::MessagePattern::Unary { selector, .. } => names.push(selector.name.clone()),
        ast::MessagePattern::Binary {
            selector,
            parameter,
        } => {
            names.push(selector.name.clone());
            names.push(parameter.name.name.clone());
        }
        ast::MessagePattern::Keyword { parts } => {
            for part in parts {
                names.push(part.keyword.name.clone());
                names.push(part.parameter.name.name.clone());
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
        // A chain of messages is walked link by link: it nests as deep as
        // the source is long.
        ast::ExpressionKind::Send(send) => {
            let (links, receiver) = send.receiver_chain();
            declared_names_of_expression(receiver, names);
            for link in links.iter().rev() {
                link.message
                    .arguments
                    .iter()
                    .for_each(|argument| declared_names_of_expression(argument, names));
            }
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
                    .map(|parameter| parameter.name.name.clone()),
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
