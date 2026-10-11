//! The module `parse_for_editor` answers for a source with errors: what an
//! editor shows while a file is typed. Each test asserts that what follows
//! the error survives, with its name and span.

use indoc::{formatdoc, indoc};
use ns_syntax::ast::{ClassBody, MessagePattern, Module, SlotDeclaration};

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
    body.methods()
        .map(|method| selector(&method.pattern))
        .collect()
}

fn class_names(body: &ClassBody) -> Vec<String> {
    body.nested_classes()
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

/// A block of `///` lines reaches the editor as one comment with its text.
#[test]
fn a_line_comment_block_is_one_comment_with_its_text() {
    let source = indoc! {"
        /// A module,
        /// documented.
        class Module = () (
            run = ( ^1 )
        ) : ()
    "};
    let parsed = ns_syntax::parse_for_editor(source);
    assert!(parsed.diagnostics.is_empty());
    let [comment] = parsed.comments.as_slice() else {
        panic!("one comment, not {}", parsed.comments.len());
    };
    assert_eq!(comment.kind, ns_syntax::CommentKind::Line);
    assert_eq!(comment.text(source), "A module,\ndocumented.");
}

/// A lexer error does not stop the parse: the editor still gets the
/// members around it.
#[test]
fn every_lexer_error_still_answers_a_module() {
    for broken in ["^#", "^#(1)", "^\u{00a7}", "^x := 1"] {
        let source = formatdoc! {"
            class Module = () (
                first = ( {broken} )
                second = ( ^2 )
            ) : ()
        "};
        let (module, messages) = recovered(&source);
        assert!(!messages.is_empty(), "{broken}");
        assert!(
            method_names(&module.root_class.instance_side).contains(&"second".to_string()),
            "{broken}: {messages:?}"
        );
    }
    for unterminated in ["^'open", "(* open"] {
        let source = formatdoc! {"
            class Module = () (
                first = ( {unterminated}
        "};
        let (_, messages) = recovered(&source);
        assert!(!messages.is_empty(), "{unterminated}");
    }
}

#[test]
fn an_empty_source_answers_a_module() {
    for source in ["", "   \n\t\n"] {
        let (module, messages) = recovered(source);
        assert!(!messages.is_empty());
        assert!(module.root_class.instance_side.methods().next().is_none());
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
    let broken = root.instance_side.methods().next().expect("a method");
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
    assert!(body
        .methods()
        .next()
        .expect("a method")
        .body
        .statements
        .is_empty());
    assert_eq!(
        body.methods().next().expect("a method").span.text(source),
        "broken = ("
    );
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
        root.instance_side
            .methods()
            .nth(1)
            .expect("a method")
            .span
            .text(source),
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
    assert_eq!(
        root.instance_side
            .methods()
            .next()
            .expect("a method")
            .body
            .statements
            .len(),
        2
    );
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
    assert_eq!(
        body.methods()
            .next()
            .expect("a method")
            .body
            .statements
            .len(),
        1
    );
    assert!(body
        .methods()
        .next()
        .expect("a method")
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
    assert_eq!(
        body.methods()
            .next()
            .expect("a method")
            .body
            .statements
            .len(),
        2
    );
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
    let inner = module
        .root_class
        .instance_side
        .nested_classes()
        .next()
        .expect("a nested class");
    assert_eq!(method_names(&inner.instance_side), ["inside", "outside"]);
    assert_eq!(
        inner
            .instance_side
            .methods()
            .nth(1)
            .expect("a method")
            .span
            .text(source),
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
    assert!(body
        .methods()
        .next()
        .expect("a method")
        .body_delimiters
        .and_then(|delimiters| delimiters.close)
        .is_some());
    let ns_syntax::ast::StatementKind::Return(ns_syntax::ast::Expression {
        kind: ns_syntax::ast::ExpressionKind::Block(block),
        ..
    }) = &body.methods().next().expect("a method").body.statements[0].kind
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
    assert_eq!(
        body.methods()
            .next()
            .expect("a method")
            .body
            .statements
            .len(),
        2
    );
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
    let ns_syntax::ast::StatementKind::Return(ns_syntax::ast::Expression {
        kind: ns_syntax::ast::ExpressionKind::DynamicArray { elements, span },
        ..
    }) = &body.methods().next().expect("a method").body.statements[0].kind
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
    let slot_names: Vec<&str> = body.slots().map(|slot| slot.name.name.as_str()).collect();
    assert_eq!(slot_names, ["first", "second"], "{messages:?}");
    // The last slot ends at its own name, not at the header's `)`.
    assert_eq!(
        body.slots().nth(1).expect("a slot").span.text(source),
        "second"
    );
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
    assert_eq!(body.slots().count(), 1, "{messages:?}");
    assert_eq!(method_names(body), ["run"], "{messages:?}");
    // `|` is a type operator too (`<E | X>`), so the annotation takes the
    // bar and stops only at the header's `)`: the section is left unclosed,
    // and each of the two is reported.
    assert_eq!(
        messages,
        [
            "unterminated `<...>` type annotation",
            "expected Bar, found RightParenthesis",
        ]
    );
    let delimiters = body.slot_delimiters.expect("the section's open bar");
    assert_eq!(delimiters.close, None);
    assert_eq!(
        body.slots().next().expect("a slot").span.text(source),
        "first <Foo |"
    );
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
    assert!(body.slots().next().is_none());
    assert!(
        body.slot_delimiters
            .and_then(|delimiters| delimiters.close)
            .is_some(),
        "{messages:?}"
    );
    assert_eq!(method_names(body), ["run"]);
}

/// A declaration whose name is missing is left out, so the resolver never
/// sees two empty names to call duplicates.
#[test]
fn a_missing_name_declares_nothing() {
    let source = indoc! {"
        class Module () (
            class Inner = outer () ()
            first = ( ^[: | 1] )
            second = ( ^self foo; )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let root = &module.root_class;
    assert_eq!(root.factory, None, "{messages:?}");
    assert_eq!(
        root.instance_side
            .nested_classes()
            .next()
            .expect("a nested class")
            .superclass,
        None
    );
    assert_eq!(method_names(&root.instance_side), ["first", "second"]);
    let ns_syntax::ast::StatementKind::Return(ns_syntax::ast::Expression {
        kind: ns_syntax::ast::ExpressionKind::Block(block),
        ..
    }) = &root
        .instance_side
        .methods()
        .next()
        .expect("a method")
        .body
        .statements[0]
        .kind
    else {
        panic!("a returned block");
    };
    assert!(block.parameters.is_empty());
    let ns_syntax::ast::StatementKind::Return(ns_syntax::ast::Expression {
        kind: ns_syntax::ast::ExpressionKind::Cascade(cascade),
        ..
    }) = &root
        .instance_side
        .methods()
        .nth(1)
        .expect("a method")
        .body
        .statements[0]
        .kind
    else {
        panic!("a returned cascade");
    };
    assert!(cascade.rest.is_empty());
}

/// The root class is the one declaration kept without a name: without it
/// there is no module, and no class holds it as a member.
#[test]
fn a_root_class_without_a_name_is_kept() {
    let source = indoc! {"
        class = () (
            run = ( ^1 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    assert_eq!(module.root_class.name.name, "", "{messages:?}");
    assert_eq!(method_names(&module.root_class.instance_side), ["run"]);
}

/// A member written inside a block is the block's content: the `|` that
/// ends the block's parameters starts no binary method `|`, and the
/// statement holding it stays whole.
#[test]
fn a_member_inside_a_block_is_none() {
    let source = indoc! {"
        class Module = () (
            first = ( ^items detect: [:each | each = ( ^each ) ] )
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
        root.instance_side
            .methods()
            .next()
            .expect("a method")
            .span
            .text(source),
        "first = ( ^items detect: [:each | each = ( ^each ) ] )"
    );
    assert_eq!(
        method_names(root.class_side.as_ref().expect("a class side")),
        ["make"]
    );
    assert_eq!(messages, ["expected an expression, found Caret"]);
}

/// An element that consumed nothing has its one diagnostic; the missing
/// `.` after it is not a second.
#[test]
fn an_empty_tuple_element_is_one_error() {
    let source = indoc! {"
        class Module = () (
            first = ( ^{ = } )
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["first", "second"], "{messages:?}");
    assert_eq!(messages, ["expected an expression, found Equals"]);
}

/// A parenthesis holding junk after its expression skips the junk and
/// takes its own `)`: the statement after it goes on, whole.
#[test]
fn junk_in_a_parenthesis_is_skipped_to_its_close() {
    let source = indoc! {"
        class Module = () (
            first = ( ^( 1 2 ) + 3 )
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(method_names(body), ["first", "second"], "{messages:?}");
    assert_eq!(
        messages,
        ["expected `)` to close the parenthesis, found Number"]
    );
    let statements = &body.methods().next().expect("a method").body.statements;
    assert_eq!(statements.len(), 1);
    let ns_syntax::ast::StatementKind::Return(ns_syntax::ast::Expression {
        kind: ns_syntax::ast::ExpressionKind::Send(send),
        ..
    }) = &statements[0].kind
    else {
        panic!("a returned send");
    };
    assert_eq!(
        send.message.selector,
        ns_syntax::ast::Selector::Binary("+".to_string())
    );
    assert!(matches!(
        send.receiver.kind,
        ns_syntax::ast::ExpressionKind::Literal(ns_syntax::ast::Literal::Integer { .. })
    ));
}

/// A `<:` clause without its receiver is left out, and the `.` after it
/// still ends the application: one error, and the member after it stays.
#[test]
fn a_mixin_clause_without_a_name_keeps_its_dot() {
    let source = indoc! {"
        class Module = () (
            class Inner = Object <: .
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let body = &module.root_class.instance_side;
    assert_eq!(class_names(body), ["Inner"], "{messages:?}");
    assert!(body
        .nested_classes()
        .next()
        .expect("a nested class")
        .mixin_applications
        .is_empty());
    assert_eq!(method_names(body), ["second"]);
    assert_eq!(
        messages,
        ["expected a name to name a superclass, found Dot"]
    );
}

/// A member in a parenthesis in a block is the block's content as well:
/// the parenthesis the statement opened is closed after it, and then the
/// block's `]`.
#[test]
fn a_member_inside_a_parenthesis_inside_a_block_is_none() {
    let source = indoc! {"
        class Module = () (
            first = ( ^items detect: [:each | (each = ( ^each )) ] )
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
        root.instance_side
            .methods()
            .next()
            .expect("a method")
            .span
            .text(source),
        "first = ( ^items detect: [:each | (each = ( ^each )) ] )"
    );
    assert_eq!(
        method_names(root.class_side.as_ref().expect("a class side")),
        ["make"]
    );
    assert_eq!(messages, ["expected an expression, found Caret"]);
}

/// A block left open before a member: the next closing bracket after the
/// member is its owner's `)`, so the member is one.
#[test]
fn a_member_after_an_open_block_is_one() {
    let source = indoc! {"
        class Module = () (
            first = ( ^items do: [:each | each foo
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
        root.instance_side
            .methods()
            .nth(1)
            .expect("a method")
            .span
            .text(source),
        "second = ( ^2 )"
    );
    assert_eq!(
        method_names(root.class_side.as_ref().expect("a class side")),
        ["make"]
    );
}

/// A lazy slot missing its `.` ends at its initializer, not at the `)`
/// after it.
#[test]
fn a_lazy_slot_without_its_dot_ends_at_its_initializer() {
    let source = indoc! {"
        class Module = () (
            lazy x = 1
        ) : ()
    "};
    let (module, messages) = recovered(source);
    let slots: Vec<&SlotDeclaration> = module.root_class.instance_side.slots().collect();
    assert_eq!(slots.len(), 1, "{messages:?}");
    assert_eq!(slots[0].span.text(source), "lazy x = 1");
}

/// A lazy slot after an unclosed body is a member the body swallowed only
/// when it parses without an error of its own, as a method or a nested
/// class must. The `.` ending it is the error of the unclosed parenthesis
/// either way: the clean slot is declared, the one whose type annotation is
/// not closed stays in the statement that swallowed it, and the method after
/// either survives.
#[test]
fn a_lazy_slot_after_an_unclosed_body_is_a_member_only_when_clean() {
    let clean = indoc! {"
        class Module = () (
            first = ( (1
            lazy cache = 1 + 2.
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(clean);
    let instance_side = &module.root_class.instance_side;
    assert_eq!(
        method_names(instance_side),
        ["first", "second"],
        "{messages:?}"
    );
    assert_eq!(instance_side.slots().count(), 1, "{messages:?}");
    assert_eq!(
        instance_side
            .slots()
            .next()
            .expect("a slot")
            .span
            .text(clean),
        "lazy cache = 1 + 2."
    );

    let broken = indoc! {"
        class Module = () (
            first = ( (1
            lazy cache <Cache = 1 + 2.
            second = ( ^2 )
        ) : ()
    "};
    let (module, messages) = recovered(broken);
    let instance_side = &module.root_class.instance_side;
    assert_eq!(
        method_names(instance_side),
        ["first", "second"],
        "{messages:?}"
    );
    assert!(instance_side.slots().next().is_none(), "{messages:?}");
}

/// Half-typed members nested twenty deep, each in a block of the one
/// around it. Every speculative member parse holds the next, and the
/// re-parse of a statement repeats the work of those nested in it, so
/// without the recovery's work budget the parse grows exponentially with
/// the depth -- in a release build 0.6 s at depth 12 and 90 s at 16 -- and
/// this test would not finish. With it the parse is linear in the source,
/// and the method around the nest survives.
#[test]
fn deeply_nested_lookalike_members_parse_in_bounded_time() {
    let depth = 20;
    let source = formatdoc! {"
        class Module = () (
            run = ( {opened}^{closed} )
            after = ( ^2 )
        ) : ()
        ",
        opened = "x = ( [ ".repeat(depth),
        closed = " ] )".repeat(depth),
    };
    let (module, messages) = recovered(&source);
    assert!(!messages.is_empty());
    assert_eq!(
        method_names(&module.root_class.instance_side),
        ["run", "after"],
        "{messages:?}"
    );
}

/// `NewspeakPrettyPrinter.ns` with the `)` of five consecutive methods
/// removed. Each body's unfinished keyword message reads the pattern after
/// it as its next part, so the methods up to the next error are the first
/// one's statement; `emit:` holds that error and is a member. Recovery
/// once spent its budget walking to the class's close for every member
/// it found, and `emit:` was swallowed as well.
#[test]
fn five_unclosed_methods_keep_the_member_after_the_message_they_continue() {
    let source = indoc! {"
        class Module = () (
            public keywordPatternNode: node = (
                visitKeywordPattern: node

            public literalPatternNode: node = (
                visitLiteralPattern: node

            public messageNode: node = (
                visitMessage: node

            public messagePatternNode: node = (
                visitMessagePattern: node

            public methodNode: node = (
                visitMethod: node

            public mixinApplication: node = (
                visitMixinApplication: node
            )
            public mixinApplicationClass: node = (
                visitMixinApplicationClass: node
            )
            emit: text = (
                output add: text.
                column:: column + text size
            )
        ) : ()
    "};
    let (module, messages) = recovered(source);
    assert_eq!(
        method_names(&module.root_class.instance_side),
        ["keywordPatternNode:", "emit:"],
        "{messages:?}"
    );
}

/// A run of twenty unclosed methods, as `KernelTests.ns`'s `FloatTests`
/// becomes with twenty `)` removed, before fifty methods more: every one
/// is kept. Recovery once walked to the class's close for every member it
/// found, and the class after the run decided how many survived.
#[test]
fn a_run_of_unclosed_methods_in_a_long_class_keeps_every_method() {
    let unclosed: String = (0..20)
        .map(|index| format!("    open{index} = ( ^{index}\n"))
        .collect();
    let closed: String = (0..50)
        .map(|index| format!("    closed{index} = ( ^{index} )\n"))
        .collect();
    let source = formatdoc! {"
        class Module = () (
        {unclosed}{closed}) : ()
    "};
    let (module, messages) = recovered(&source);
    let names = method_names(&module.root_class.instance_side);
    assert_eq!(names.len(), 70, "{names:?}");
    assert_eq!(names[19], "open19");
    assert_eq!(names[69], "closed49");
    assert_eq!(messages.len(), 20, "{messages:?}");
}

/// `MinitestTests.ns` with twenty `)` removed from its first nested test
/// contexts on: a run of unclosed methods through a class side and into
/// the next class. Its statements ask again and again for the same
/// members, and parsed afresh each time the run cost work cubic in its
/// length and spent the budget before `createFailureResultFor:description:`
/// was found.
#[test]
fn a_run_of_unclosed_methods_through_a_class_side_keeps_its_members() {
    let source = indoc! {"
        class Module = () (
            class Context = () (
                public testErrorCreation = (
                    assert: [ 1 zork ]

                public testFailureCreation = (
                    assert: false description: 'better not see this'

                public testSuccess = (
                    assert: true

                public createFailureResultFor: testCase <TestCase> description: description <String> ^<TestFailure> = (
                    ^TestFailure case: testCase description: 'created here'

            ) : (
                TEST_CONTEXT = (


            public class ShouldShouldntTestContext new = TestContext new (
            ) (
                public testShouldFail = (
                    [ should: [ TestException1 new signal ] signal: TestException2 ] on: TestException1
                        do: [:exception | exception resume: nil ]

                public testShouldFailWithDescription = (
                    [
                        should: [ TestException1 new signal ]
                            signal: TestException2
                            description: 'custom description'
                    ] on: TestException1 do: [:exception | exception resume: nil ]

                public testShouldPassExceptionSubclass = (
                    should: [ TestException1 new signal ] signal: Exception

                public testShouldPassSameExceptionClass = (
                    should: [ TestException1 new signal ] signal: TestException1

                public testShouldntFail = (
                    shouldnt: [ TestException1 new signal ] signal: TestException1

                public testShouldntFailWithDescription = (
                    shouldnt: [ TestException1 new signal ]
                        signal: TestException1
                        description: 'custom description'

                public testShouldntPass = (
                    [ shouldnt: [ TestException1 new signal ] signal: TestException2 ] on: TestException1
                        do: [:exception | exception resume: nil ]

            ) : (
                TEST_CONTEXT = (


        ) : ()
    "};
    let (module, messages) = recovered(source);
    let context = module
        .root_class
        .instance_side
        .nested_classes()
        .next()
        .expect("a nested class");
    assert_eq!(
        method_names(&context.instance_side),
        [
            "testErrorCreation",
            "createFailureResultFor:description:",
            "TEST_CONTEXT"
        ],
        "{messages:?}"
    );
}

/// A slot whose initializer lacks its `.` before the header's `)` ends at
/// the initializer.
#[test]
fn a_slot_initializer_without_its_dot_ends_at_the_initializer() {
    let source = indoc! {"
        class Module = (
            | a = 1
        ) () : ()
    "};
    let (module, messages) = recovered(source);
    let slots: Vec<&SlotDeclaration> = module.root_class.instance_side.slots().collect();
    assert_eq!(slots.len(), 1, "{messages:?}");
    assert_eq!(slots[0].span.text(source), "a = 1");
}

/// Left as it is: an unfinished body whose next line is a valid expression
/// is read as that expression. `bar = ( 2 )` is a comparison, so `bar` is
/// no method, and the missing `)` is reported at `baz`, which survives.
#[test]
fn an_unfinished_body_before_a_valid_expression_reads_it_as_one() {
    let source = indoc! {"
        class Module = () (
            first = (
                x foo.
                bar = ( 2 )
            baz = ( ^3 )
        ) : (
            make = ( ^self new )
        )
    "};
    let (module, messages) = recovered(source);
    let root = &module.root_class;
    assert_eq!(
        method_names(&root.instance_side),
        ["first", "baz"],
        "{messages:?}"
    );
    let class_side = root.class_side.as_ref().expect("the class side survives");
    assert_eq!(method_names(class_side), ["make"]);
    assert_eq!(
        messages,
        ["expected `)` to end the method body, found Identifier"]
    );
}

/// Left as it is: half a keyword pattern above a method is a valid method
/// whose parameter is the next method's name, `foo: bar`.
#[test]
fn half_a_keyword_pattern_above_a_method_is_a_keyword_method() {
    let source = indoc! {"
        class Module = () (
            foo:
            bar = ( ^2 )
        ) : (
            make = ( ^self new )
        )
    "};
    let (module, messages) = recovered(source);
    let root = &module.root_class;
    assert_eq!(method_names(&root.instance_side), ["foo:"], "{messages:?}");
    let class_side = root.class_side.as_ref().expect("the class side survives");
    assert_eq!(method_names(class_side), ["make"]);
    assert!(messages.is_empty(), "{messages:?}");
}

/// A run of unclosed methods far longer than `MAX_DEPTH` keeps only its
/// first method, whose statement takes the rest as one chain of sends, a
/// link per method: `1 m = ( … ) m = ( … ) …`. The chain is built in a
/// loop, but each link holds the one before it, so it nests as deep as the
/// run is long, and the speculative member parses dropped such chains by
/// recursion: 10,000 methods overflowed this stack, and 60,000 the
/// language server's 8 MiB. The chain is dropped link by link now, so the
/// stack the parse needs does not grow with the source.
#[test]
fn a_run_of_unclosed_methods_far_past_the_depth_limit_parses_on_a_small_stack() {
    const METHODS: usize = 10_000;
    const STACK_BYTES: usize = 512 * 1024;
    let source = formatdoc! {"
        class Module = () (
        {unclosed}",
        unclosed = "    m = ( ^1\n".repeat(METHODS),
    };
    let (names, message_count) = std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(move || {
            let (module, messages) = recovered(&source);
            (
                method_names(&module.root_class.instance_side),
                messages.len(),
            )
        })
        .expect("spawn the parsing thread")
        .join()
        .expect("the parse answers on a small stack");
    assert_eq!(names, ["m"]);
    assert_eq!(message_count, METHODS + 1);
}
