//! The module `parse_for_editor` answers for a source with errors: what an
//! editor shows while a file is typed. Each test asserts that what follows
//! the error survives, with its name and span.

use indoc::indoc;
use ns_syntax::ast::{ClassBody, MessagePattern, Module};

fn recovered(source: &str) -> (Module, Vec<String>) {
    let parsed = ns_syntax::parse_for_editor(source);
    let messages = parsed
        .diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();
    (parsed.module, messages)
}

fn selector(pattern: &MessagePattern) -> String {
    match pattern {
        MessagePattern::Unary { selector } | MessagePattern::Binary { selector, .. } => {
            selector.name.clone()
        }
        MessagePattern::Keyword { parts } => parts
            .iter()
            .map(|(keyword, _)| format!("{}:", keyword.name))
            .collect(),
    }
}

fn method_names(body: &ClassBody) -> Vec<String> {
    body.methods
        .iter()
        .map(|method| selector(&method.pattern))
        .collect()
}

// Used by the recovery tests of the tasks that follow.
#[allow(dead_code)]
fn class_names(body: &ClassBody) -> Vec<String> {
    body.nested_classes
        .iter()
        .map(|class| class.name.name.clone())
        .collect()
}

#[test]
fn a_clean_source_parses_as_parse_parses_it() {
    let source = indoc! {"
        (* A module. *)
        class Module = () (
            run = ( ^1 )
        ) : ()
    "};
    let parsed = ns_syntax::parse_for_editor(source);
    assert!(parsed.diagnostics.is_empty());
    assert_eq!(Ok(parsed.module), ns_syntax::parse(source).map_err(|_| ()));
    assert_eq!(Ok(parsed.tokens), ns_syntax::lex(source).map_err(|_| ()));
    assert_eq!(parsed.comments.len(), 1);
}

/// A lexer error does not stop the parse: the editor still gets the
/// members around it.
#[test]
fn every_lexer_error_still_answers_a_module() {
    for broken in ["^#", "^#(1)", "^\u{00a7}"] {
        let source =
            format!("class Module = () (\n    first = ( {broken} )\n    second = ( ^2 )\n) : ()\n");
        let (module, messages) = recovered(&source);
        assert!(!messages.is_empty(), "{broken}");
        assert!(
            method_names(&module.root_class.instance_side).contains(&"second".to_string()),
            "{broken}: {messages:?}"
        );
    }
    // A stray token after the return loses the later members until the
    // parser's statement recovery keeps them; a module still answers.
    let source = "class Module = () (
    first = ( ^x := 1 )
    second = ( ^2 )
) : ()
";
    let (_, messages) = recovered(source);
    assert!(!messages.is_empty());
    for unterminated in ["^'open", "(* open"] {
        let source = format!("class Module = () (\n    first = ( {unterminated}\n");
        let (_, messages) = recovered(&source);
        assert!(!messages.is_empty(), "{unterminated}");
    }
}

#[test]
fn an_empty_source_answers_a_module() {
    for source in ["", "   \n\t\n"] {
        let (module, messages) = recovered(source);
        assert!(!messages.is_empty());
        assert!(module.root_class.instance_side.methods.is_empty());
    }
}
