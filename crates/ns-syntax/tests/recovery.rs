//! The module `parse_for_editor` answers for a source with errors: what an
//! editor shows while a file is typed. Each test asserts that what follows
//! the error survives, with its name and span.

use indoc::{formatdoc, indoc};
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
    for broken in ["^#", "^#(1)", "^\u{00a7}", "^x := 1"] {
        let source =
            format!("class Module = () (\n    first = ( {broken} )\n    second = ( ^2 )\n) : ()\n");
        let (module, messages) = recovered(&source);
        assert!(!messages.is_empty(), "{broken}");
        assert!(
            method_names(&module.root_class.instance_side).contains(&"second".to_string()),
            "{broken}: {messages:?}"
        );
    }
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

/// The case that matters most while typing: a method whose body is not
/// closed yet keeps every member after it.
#[test]
fn a_half_typed_method_keeps_the_members_after_it() {
    let source = indoc! {"
        class Module = () (
            broken = (
                ^1
            second = (
                ^2
            )
            class Inner = () ()
        ) : (
            make = ( ^self new )
        )
    "};
    let (module, messages) = recovered(source);
    let root = &module.root_class;
    assert_eq!(
        method_names(&root.instance_side),
        ["broken", "second"],
        "{messages:?}"
    );
    assert_eq!(class_names(&root.instance_side), ["Inner"]);
    let class_side = root.class_side.as_ref().expect("the class side survives");
    assert_eq!(method_names(class_side), ["make"]);
    let broken = &root.instance_side.methods[0];
    assert_eq!(
        broken
            .body_delimiters
            .and_then(|delimiters| delimiters.close),
        None
    );
    assert!(
        broken.span.text(source).ends_with("^1"),
        "{}",
        broken.span.text(source)
    );
    assert_eq!(messages.len(), 1, "{messages:?}");
}

/// A body not begun yet: the member after it is the statement's own first
/// token, so the body is empty and only its missing `)` is reported.
#[test]
fn an_empty_unclosed_method_keeps_the_members_after_it() {
    let source = indoc! {"
        class Module = () (
            broken = (
            second = (
                ^2
            )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["broken", "second"], "{messages:?}");
    assert!(body.methods[0].body.statements.is_empty());
    assert_eq!(body.methods[0].span.text(source), "broken = (");
    assert_eq!(messages.len(), 1, "{messages:?}");
}

/// A statement in a block, itself in a statement, each re-parsed up to the
/// same member start: the bound each sets is lifted again, so the member
/// and everything after it are read in full.
#[test]
fn an_unclosed_block_in_an_unclosed_method_keeps_the_members_after_it() {
    let source = indoc! {"
        class Module = () (
            first = ( ^[ :each | each
            second = ( ^2 )
            class Inner = () ()
        ) : (
            make = ( ^self new )
        )
    "};
    let (module, messages) = recovered(source);
    let root = &module.root_class;
    assert_eq!(
        method_names(&root.instance_side),
        ["first", "second"],
        "{messages:?}"
    );
    assert_eq!(
        root.instance_side.methods[1].span.text(source),
        "second = ( ^2 )"
    );
    assert_eq!(class_names(&root.instance_side), ["Inner"]);
    assert_eq!(
        method_names(root.class_side.as_ref().expect("a class side")),
        ["make"]
    );
    assert_eq!(
        messages,
        [
            "expected RightBracket, found the start of the next member",
            "expected `)` to end the method body, found Identifier",
        ]
    );
}

/// `name = (expression)` is a comparison as much as a member's start. A
/// statement whose error lies after such a comparison, or inside one that
/// is no clean member, swallowed no member: nothing is invented, and the
/// members after it stay, as without the re-parse.
#[test]
fn a_comparison_that_reads_as_a_member_is_not_one() {
    for statement in [
        "(count = (limit + 1)) ifTrue: [ foo: ]. ^count",
        "^items detect: [:each | each size = (limit + 1)] ifNone: [ foo: ]",
    ] {
        let source = formatdoc! {"
            class Module = () (
                first = ( {statement} )
                second = ( ^2 )
            ) : (
                make = ( ^self new )
            )
        "};
        let (module, messages) = recovered(&source);
        let root = &module.root_class;
        assert_eq!(
            method_names(&root.instance_side),
            ["first", "second"],
            "{statement}: {messages:?}"
        );
        let class_side = root.class_side.as_ref().expect("a class side");
        assert_eq!(method_names(class_side), ["make"], "{statement}");
        assert_eq!(messages.len(), 1, "{statement}: {messages:?}");
    }
}

/// `x = (1 + )` fails as a member itself, so it is none: no method `x` is
/// invented, and the members after it stay.
#[test]
fn a_comparison_that_fails_as_a_member_invents_none() {
    let source = indoc! {"
        class Module = () (
            first = ( x = (1 + ). ^x )
            second = ( ^2 )
        ) : (
            make = ( ^self new )
        )
    "};
    let (module, messages) = recovered(source);
    let root = &module.root_class;
    assert_eq!(
        method_names(&root.instance_side),
        ["first", "second"],
        "{messages:?}"
    );
    assert_eq!(
        method_names(root.class_side.as_ref().expect("a class side")),
        ["make"],
        "{messages:?}"
    );
    assert_eq!(root.instance_side.methods[0].body.statements.len(), 2);
    assert_eq!(messages.len(), 1, "{messages:?}");
}

/// An ambiguous source: `+ second = ( ^2 )` and `at: second = ( ^2 )` are
/// members themselves, a binary and a keyword method, so the half-typed
/// send's selector starts the member that holds the error and `second`
/// becomes its parameter.
#[test]
fn a_half_typed_send_before_a_member_starts_the_member_with_its_selector() {
    for (statement, member) in [("^1 +", "+"), ("^dict at:", "at:")] {
        let source = formatdoc! {"
            class Module = () (
                first = ( {statement}
                second = ( ^2 )
            ) : ()
        "};
        let (module, messages) = recovered(&source);
        assert_eq!(
            method_names(&module.root_class.instance_side),
            ["first", member],
            "{statement}: {messages:?}"
        );
        assert_eq!(messages.len(), 1, "{statement}: {messages:?}");
    }
}

/// A bare `= (` has no message pattern, so it starts no method and no
/// method with an empty selector is invented for it.
#[test]
fn an_equals_sign_without_a_pattern_starts_no_method() {
    let source = indoc! {"
        class Module = () (
            = ( ^1 )
            first = ( ^1 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    assert_eq!(method_names(&module.root_class.instance_side), ["first"]);
    assert_eq!(messages.len(), 1, "{messages:?}");
}

#[test]
fn a_statement_after_a_return_is_skipped_to_the_end_of_the_body() {
    let source = indoc! {"
        class Module = () (
            first = ( ^1. 2 + 3 )
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["first", "second"], "{messages:?}");
    assert_eq!(body.methods[0].body.statements.len(), 1);
    assert!(body.methods[0]
        .body_delimiters
        .and_then(|delimiters| delimiters.close)
        .is_some());
    assert_eq!(messages.len(), 1, "{messages:?}");
}

#[test]
fn junk_between_statements_is_skipped_to_the_next_statement() {
    let source = indoc! {"
        class Module = () (
            first = ( | x | x:: 1 'stray'. ^x )
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["first", "second"], "{messages:?}");
    assert_eq!(body.methods[0].body.statements.len(), 2);
    assert_eq!(messages.len(), 1, "{messages:?}");
}

/// A run of tokens no member starts with is one error, not one per token.
#[test]
fn a_run_of_junk_in_a_class_side_is_one_error() {
    let source = indoc! {"
        class Module = () (
            3 4 (5 6) 7
            first = ( ^1 )
        ) : (
            8 9
            make = ( ^self new )
        )
    "};
    let (module, messages) = recovered(source);
    let root = &module.root_class;
    assert_eq!(method_names(&root.instance_side), ["first"]);
    assert_eq!(
        method_names(root.class_side.as_ref().expect("a class side")),
        ["make"]
    );
    assert_eq!(messages.len(), 2, "{messages:?}");
}

/// Nothing but indentation says where a nested class missing its `)`
/// ends, so the members after it become its own; they survive all the
/// same, with their spans.
#[test]
fn a_nested_class_missing_its_close_keeps_the_members_after_it() {
    let source = indoc! {"
        class Module = () (
            class Inner = () (
                inside = ( ^1 )
            outside = ( ^2 )
        ) : ()
    "};
    let (module, _) = recovered(source);
    let inner = &module.root_class.instance_side.nested_classes[0];
    assert_eq!(method_names(&inner.instance_side), ["inside", "outside"]);
    assert_eq!(
        inner.instance_side.methods[1].span.text(source),
        "outside = ( ^2 )"
    );
}

#[test]
fn an_unclosed_block_ends_at_the_method_close() {
    let source = indoc! {"
        class Module = () (
            first = ( ^[:each | each
            )
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["first", "second"], "{messages:?}");
    assert!(body.methods[0]
        .body_delimiters
        .and_then(|delimiters| delimiters.close)
        .is_some());
    let ns_syntax::ast::Statement::Return(ns_syntax::ast::Expression::Block(block)) =
        &body.methods[0].body.statements[0]
    else {
        panic!("a returned block");
    };
    assert_eq!(
        block.delimiters.and_then(|delimiters| delimiters.close),
        None
    );
    assert!(
        block.span.text(source).ends_with("each"),
        "{}",
        block.span.text(source)
    );
}

#[test]
fn an_unclosed_parenthesis_stops_at_the_end_of_its_statement() {
    let source = indoc! {"
        class Module = () (
            first = ( | x | x:: (1 + 2. ^x )
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["first", "second"], "{messages:?}");
    assert_eq!(body.methods[0].body.statements.len(), 2);
    assert_eq!(messages.len(), 1, "{messages:?}");
}

#[test]
fn a_tuple_with_junk_keeps_its_elements_and_its_close() {
    let source = indoc! {"
        class Module = () (
            first = ( ^{ 1. 2 3. 4 } )
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["first", "second"], "{messages:?}");
    let ns_syntax::ast::Statement::Return(ns_syntax::ast::Expression::DynamicArray {
        elements,
        span,
    }) = &body.methods[0].body.statements[0]
    else {
        panic!("a returned tuple");
    };
    assert_eq!(elements.len(), 3);
    assert_eq!(span.text(source), "{ 1. 2 3. 4 }");
}

#[test]
fn an_unclosed_slot_section_stops_at_the_header_close() {
    let source = indoc! {"
        class Module = (
            | first second
        ) (
            run = ( ^first )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    let slot_names: Vec<&str> = body
        .slots
        .iter()
        .map(|slot| slot.name.name.as_str())
        .collect();
    assert_eq!(slot_names, ["first", "second"], "{messages:?}");
    assert_eq!(
        body.slot_delimiters.and_then(|delimiters| delimiters.close),
        None
    );
    assert_eq!(method_names(body), ["run"]);
}

/// A type annotation's scan ends at a bracket the annotation did not open,
/// rather than taking it and the rest of the class with it.
#[test]
fn a_half_typed_annotation_does_not_swallow_the_class() {
    let source = indoc! {"
        class Module = (
            | first <Foo |
        ) (
            run = ( ^first )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(body.slots.len(), 1, "{messages:?}");
    assert_eq!(method_names(body), ["run"], "{messages:?}");
}

#[test]
fn half_a_keyword_message_stays_inside_its_statement() {
    let source = indoc! {"
        class Module = () (
            first = ( ^self at: )
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["first", "second"], "{messages:?}");
    assert_eq!(messages.len(), 1, "{messages:?}");
}

/// `| public |` declares no slot; the access modifier takes `public`, and
/// the section's closing bar must stay the close.
#[test]
fn a_slot_section_of_a_modifier_alone_keeps_its_close() {
    let source = indoc! {"
        class Module = (
            | public |
        ) (
            run = ( ^1 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert!(body.slots.is_empty());
    assert!(
        body.slot_delimiters
            .and_then(|delimiters| delimiters.close)
            .is_some(),
        "{messages:?}"
    );
    assert_eq!(method_names(body), ["run"]);
}
