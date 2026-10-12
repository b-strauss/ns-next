use indoc::indoc;
use ns_syntax::ast::*;

fn parsed(source: &str) -> Module {
    match ns_syntax::parse(source) {
        Ok(module) => module,
        Err(diagnostics) => panic!("{diagnostics:?}"),
    }
}

fn lines_of(comment: &AttachedComment) -> Vec<&str> {
    comment.lines.iter().map(String::as_str).collect()
}

/// The expression the first statement of the root class's first method
/// returns.
fn returned_expression(module: &Module) -> &Expression {
    let method = module
        .root_class
        .instance_side
        .methods()
        .next()
        .expect("a method");
    let StatementKind::Return(value) = &method.body.statements[0].kind else {
        panic!("a return");
    };
    value
}

#[test]
fn a_comment_above_a_member_leads_it() {
    let module = parsed(indoc! {"
        class A = () (
            /// Answers one.
            public one = ( ^1 )
        )
    "});
    let method = module
        .root_class
        .instance_side
        .methods()
        .next()
        .expect("one");
    assert_eq!(method.comments.len(), 1);
    let comment = &method.comments[0];
    assert_eq!(comment.anchor, Anchor::Before);
    assert_eq!(comment.placement, Placement::OwnLine);
    assert_eq!(lines_of(comment), [" Answers one."]);
}

#[test]
fn a_comment_after_a_statement_on_its_line_trails_it() {
    let module = parsed(indoc! {"
        class A = () (
            public m = (
                count:: 0. /// restart
                ^count
            )
        )
    "});
    let method = module.root_class.instance_side.methods().next().expect("m");
    let first = &method.body.statements[0];
    assert_eq!(first.comments.len(), 1);
    assert_eq!(first.comments[0].anchor, Anchor::After);
    assert_eq!(first.comments[0].placement, Placement::EndOfLine);
}

#[test]
fn a_comment_after_the_last_member_dangles_in_the_side() {
    let module = parsed(indoc! {"
        class A = () (
            public m = ( ^1 )

            (* the end *)
        )
    "});
    let side = &module.root_class.instance_side;
    let method = side.methods().next().expect("m");
    assert!(method.comments.is_empty());
    assert_eq!(side.comments.len(), 1);
    assert!(matches!(side.comments[0].anchor, Anchor::Inner(_)));
}

#[test]
fn metadata_between_a_pattern_and_its_equals_sign_is_inner() {
    let module = parsed(indoc! {"
        class A = () (
            public foo: x (* :exemplar: A new foo: 3 *) = ( ^x )
        )
    "});
    let method = module
        .root_class
        .instance_side
        .methods()
        .next()
        .expect("foo:");
    assert_eq!(method.comments.len(), 1);
    assert!(matches!(method.comments[0].anchor, Anchor::Inner(_)));
    assert_eq!(method.comments[0].placement, Placement::Inline);
}

#[test]
fn a_comment_between_a_keyword_and_its_argument_leads_the_argument() {
    let module = parsed(indoc! {"
        class A = () (
            public m = ( ^self at: (* index *) 1 put: 2 )
        )
    "});
    let method = module.root_class.instance_side.methods().next().expect("m");
    let StatementKind::Return(value) = &method.body.statements[0].kind else {
        panic!("a return");
    };
    let ExpressionKind::Send(send) = &value.kind else {
        panic!("at:put:");
    };
    let argument = &send.message.arguments[0];
    assert_eq!(argument.comments.len(), 1);
    assert_eq!(argument.comments[0].anchor, Anchor::Before);
    assert_eq!(argument.comments[0].placement, Placement::Inline);
}

#[test]
fn a_block_comment_keeps_its_lines_and_column() {
    let module = parsed("class A = () (\n    (* one\n       two *)\n    public m = ()\n)\n");
    let method = module.root_class.instance_side.methods().next().expect("m");
    let comment = &method.comments[0];
    assert_eq!(comment.column, 4);
    assert_eq!(lines_of(comment), ["(* one", "       two *)"]);
}

#[test]
fn the_class_comment_is_the_first_comment_in_the_header() {
    let module = parsed(indoc! {"
        class A = (
            (* A counter. *)
            |
              count ::= 0.
            |
        ) ()
    "});
    let comment = module.root_class.class_comment().expect("a class comment");
    assert_eq!(lines_of(comment), ["(* A counter. *)"]);
}

#[test]
fn a_comment_inside_empty_bars_keeps_them() {
    let module = parsed("class A = ( | (* none yet *) | ) ()");
    assert!(module.root_class.instance_side.slot_delimiters.is_some());
}

#[test]
fn the_editor_parse_attaches_like_parse() {
    let source = indoc! {"
        class A = () (
            /// one
            public one = ( ^1 ) (* trailing *)
        )
    "};
    let editor = ns_syntax::parse_for_editor(source);
    assert_eq!(editor.module, parsed(source));
}

#[test]
fn without_positions_equates_layouts_that_say_the_same() {
    let narrow = parsed("class A = () (\npublic m = ( ^1 ) )");
    let wide = parsed("class A = () (\n        public m = (\n            ^1\n        )\n)\n");
    assert_eq!(narrow.without_positions(), wide.without_positions());
}

#[test]
fn a_long_chain_attaches_on_a_small_stack() {
    let mut source = String::from("class A = () (\npublic m = ( ^1");
    for _ in 0..100_000 {
        source.push_str(" + 1");
    }
    source.push_str(" (* end *) )\n)\n");
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let module = parsed(&source);
            drop(module.without_positions());
        })
        .expect("spawn")
        .join()
        .expect("attaching and erasing a long chain stays on a small stack");
}

#[test]
fn a_comment_right_after_the_caret_leads_the_returned_expression() {
    let module = parsed(indoc! {"
        class A = () (
            public m = ( ^ (* why *) 1 )
        )
    "});
    let value = returned_expression(&module);
    assert_eq!(value.comments.len(), 1);
    assert_eq!(value.comments[0].anchor, Anchor::Before);
    assert_eq!(value.comments[0].placement, Placement::Inline);
}

#[test]
fn a_comment_inside_a_type_annotation_is_inner_on_the_slot() {
    let module = parsed(indoc! {"
        class A = (
            | count <Integer (* small *)> ::= 0. |
        ) ()
    "});
    let slot = &module.root_class.instance_side.header_slots[0];
    assert_eq!(slot.comments.len(), 1);
    // `count`, `<` and `Integer` stand before it.
    assert_eq!(slot.comments[0].anchor, Anchor::Inner(3));
}

#[test]
fn a_comment_in_an_empty_block_is_inner_on_the_block() {
    let module = parsed(indoc! {"
        class A = () (
            public m = ( ^[ (* nothing *) ] )
        )
    "});
    let ExpressionKind::Block(block) = &returned_expression(&module).kind else {
        panic!("a block");
    };
    assert_eq!(block.comments.len(), 1);
    assert_eq!(block.comments[0].anchor, Anchor::Inner(1));
    assert!(block.body.comments.is_empty());
}

#[test]
fn a_comment_before_the_class_leads_the_root_class() {
    let module = parsed(indoc! {"
        Newspeak3
        (* About A. *)
        class A = () ()
    "});
    let comments = &module.root_class.comments;
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].anchor, Anchor::Before);
    assert_eq!(comments[0].placement, Placement::OwnLine);
    assert!(module.comments.is_empty());
}

#[test]
fn a_comment_before_the_language_id_is_inner_on_the_module() {
    let module = parsed(indoc! {"
        (* A module. *)
        Newspeak3
        class A = () ()
    "});
    assert_eq!(module.comments.len(), 1);
    assert_eq!(module.comments[0].anchor, Anchor::Inner(0));
}

#[test]
fn a_class_side_comment_is_kept() {
    let module = parsed(indoc! {"
        class A = () () : (
            (* The class side. *)
        )
    "});
    let side = module.root_class.class_side.as_ref().expect("a class side");
    assert_eq!(side.comments.len(), 1);
    assert_eq!(lines_of(&side.comments[0]), ["(* The class side. *)"]);
    assert_eq!(side.comments[0].anchor, Anchor::Inner(1));
}

#[test]
fn a_recovered_parse_attaches_without_panicking() {
    let editor = ns_syntax::parse_for_editor("class A = ( (* x *) public m = ( ^");
    assert!(!editor.diagnostics.is_empty());
    assert_eq!(editor.comments.len(), 1);
    // The recovery reads `public m = ( ^` as an initializer expression.
    let side = &editor.module.root_class.instance_side;
    let statement = &side.initializer_expressions[0];
    assert_eq!(statement.comments.len(), 1);
    assert_eq!(statement.comments[0].anchor, Anchor::Before);
}

#[test]
fn comments_at_type_arguments_stay_with_their_owner() {
    let module = parsed(indoc! {"
        class A = () (
            public m = ( ^x foo[E (* inside *)] (* after *) bar )
        )
    "});
    let ExpressionKind::Send(outer) = &returned_expression(&module).kind else {
        panic!("x foo[E] bar");
    };
    let receiver = &outer.receiver;
    assert_eq!(receiver.comments.len(), 1);
    assert_eq!(lines_of(&receiver.comments[0]), ["(* after *)"]);
    assert_eq!(receiver.comments[0].anchor, Anchor::After);
    let ExpressionKind::Send(inner) = &receiver.kind else {
        panic!("x foo[E]");
    };
    assert_eq!(inner.message.comments.len(), 1);
    // `foo`, `[` and `E` stand before it.
    assert_eq!(inner.message.comments[0].anchor, Anchor::Inner(3));
}

#[test]
fn a_comment_in_a_value_site_type_argument_is_inner_on_its_expression() {
    let module = parsed(indoc! {"
        class A = () (
            public m = ( ^(Box)[Integer (* why *)] new )
        )
    "});
    let ExpressionKind::Send(send) = &returned_expression(&module).kind else {
        panic!("a send");
    };
    // `(`, `Box`, `)`, `[` and `Integer` stand before it.
    assert_eq!(send.receiver.comments.len(), 1);
    assert_eq!(send.receiver.comments[0].anchor, Anchor::Inner(5));
}

#[test]
fn a_comment_inside_a_pattern_literal_stays_in_its_text() {
    let module = parsed(indoc! {"
        class A = () (
            public m = ( ^<at: (* any *) 1> )
        )
    "});
    let value = returned_expression(&module);
    let ExpressionKind::Literal(Literal::Pattern { text, .. }) = &value.kind else {
        panic!("a pattern");
    };
    assert_eq!(text, "<at: (* any *) 1>");
    assert!(value.comments.is_empty());
}

#[test]
fn a_remark_after_the_open_parenthesis_is_not_the_class_comment() {
    let module = parsed(indoc! {"
        class C = ( /// a
            /// b
        ) ()
    "});
    let comment = module.root_class.class_comment().expect("a class comment");
    assert_eq!(lines_of(comment), [" b"]);
}

#[test]
fn a_comment_after_the_initializer_expressions_is_no_class_comment() {
    let module = parsed(indoc! {"
        class C = ( | | x.
            (* after x *)
        ) ()
    "});
    assert!(module.root_class.class_comment().is_none());
    let module = parsed(indoc! {"
        class C = ( | | (* before x *) x. ) ()
    "});
    let comment = module.root_class.class_comment().expect("a class comment");
    assert_eq!(lines_of(comment), ["(* before x *)"]);
}

#[test]
fn a_leading_string_is_no_class_comment() {
    let module = parsed(indoc! {"
        class C = ( 'hello' printLine. ) ()
    "});
    assert!(module.root_class.class_comment().is_none());
}

#[test]
fn without_positions_drops_empty_parts() {
    let written = parsed("class A = ( | | ) ( m = ( | | ^1 ) ) : ( )");
    let printed = parsed("class A = () ( m = ( ^1 ) )");
    assert_eq!(written.without_positions(), printed.without_positions());
}

#[test]
fn without_positions_keeps_a_class_side_holding_a_comment() {
    let module = parsed("class A = () () : ( (* none yet *) )");
    assert!(module.without_positions().root_class.class_side.is_some());
}

#[test]
fn without_positions_keeps_empty_bars_holding_a_comment() {
    let module = parsed("class A = ( | (* none yet *) | ) ( m = ( | (* none *) | ) )");
    let erased = module.without_positions();
    assert!(erased.root_class.instance_side.slot_delimiters.is_some());
    let method = erased.root_class.instance_side.methods().next().expect("m");
    assert!(method.body.temporaries_delimiters.is_some());
}

#[test]
fn without_positions_renumbers_after_dropped_bars() {
    let written = parsed("class A = ( | | (* c *) ) ()");
    let printed = parsed("class A = ( (* c *) ) ()");
    assert_eq!(written.without_positions(), printed.without_positions());
}

#[test]
fn without_positions_moves_block_comment_lines_with_their_column() {
    let deep =
        parsed("class A = () (\n        (* one\n           two\n      three *)\n    m = ()\n)\n");
    let shallow = parsed("class A = () (\n  (* one\n     two\n three *)\n  m = ()\n)\n");
    assert_eq!(deep.without_positions(), shallow.without_positions());
}

/// `source` parsed, its layout erased.
fn without_layout(source: &str) -> Module {
    parsed(source).without_positions()
}

#[test]
fn a_trailing_comment_in_a_one_line_block_keeps_its_owner_without_the_period() {
    let with_period = parsed("class A = () ( m = ( ^[x (* c *).] ) )");
    let without_period = parsed("class A = () ( m = ( ^[x (* c *)] ) )");
    for module in [&with_period, &without_period] {
        let ExpressionKind::Block(block) = &returned_expression(module).kind else {
            panic!("a block");
        };
        assert_eq!(block.body.comments.len(), 1);
        assert_eq!(block.body.comments[0].anchor, Anchor::After);
        assert!(block.body.statements[0].comments.is_empty());
    }
    assert_eq!(
        with_period.without_positions(),
        without_period.without_positions()
    );
}

#[test]
fn a_trailing_comment_on_the_last_statement_keeps_its_owner_either_side_of_the_period() {
    let before = parsed("class A = () ( m = ( ^1 (* c *). ) )");
    let after = parsed("class A = () ( m = (\n  ^1. (* c *)\n) )");
    for module in [&before, &after] {
        let method = module.root_class.instance_side.methods().next().expect("m");
        assert_eq!(method.body.comments.len(), 1);
        assert_eq!(method.body.comments[0].anchor, Anchor::After);
    }
    assert_eq!(before.without_positions(), after.without_positions());
}

#[test]
fn a_trailing_comment_on_a_slot_keeps_its_owner_either_side_of_the_period() {
    let before = parsed("class A = ( | x ::= 0 (* c *). | ) ()");
    let after = parsed("class A = (\n  |\n    x ::= 0. (* c *)\n  |\n) ()");
    for module in [&before, &after] {
        let slot = &module.root_class.instance_side.header_slots[0];
        assert_eq!(slot.comments.len(), 1);
        assert_eq!(slot.comments[0].anchor, Anchor::After);
    }
    assert_eq!(before.without_positions(), after.without_positions());
}

#[test]
fn placement_is_layout_and_erased() {
    assert_eq!(
        without_layout("class A = () ( m = ( ^1 (* c *) ) )"),
        without_layout("class A = () (\n  m = (\n    ^1. (* c *)\n  )\n)")
    );
}

#[test]
fn anchors_survive_reformatting() {
    for (written, reformatted) in [
        // A comment on its own line before a statement leads it, however
        // it is indented.
        (
            "class A = () ( m = ( a.\n(* c *)\nb ) )",
            "class A = () (\n  m = (\n    a.\n    (* c *)\n    b.\n  )\n)",
        ),
        // A comment at the end of a statement's line trails it.
        (
            "class A = () ( m = ( a. (* c *)\nb ) )",
            "class A = () (\n  m = (\n    a. (* c *)\n    b.\n  )\n)",
        ),
        // A one-line block opened up.
        (
            "class A = () ( m = ( ^[:each | each foo (* c *)] ) )",
            "class A = () (\n  m = (\n    ^[:each |\n      each foo. (* c *)\n    ].\n  )\n)",
        ),
        // A dangling comment in a block, with and without the period.
        (
            "class A = () ( m = ( ^[x.\n(* c *)] ) )",
            "class A = () (\n  m = (\n    ^[\n      x\n      (* c *)\n    ].\n  )\n)",
        ),
    ] {
        assert_eq!(
            without_layout(written),
            without_layout(reformatted),
            "{written:?}"
        );
    }
}

/// A comment beside a cascade's `;`, before or after it, trails the message
/// the `;` ends, as one beside a statement's `.` trails the statement.
#[test]
fn a_comment_beside_a_cascades_semicolon_trails_the_message_before_it() {
    let before = parsed("class A = () ( m = ( ^x foo (* c *); bar ) )");
    let after = parsed("class A = () ( m = ( ^x foo; (* c *) bar ) )");
    for module in [&before, &after] {
        let ExpressionKind::Cascade(cascade) = &returned_expression(module).kind else {
            panic!("a cascade");
        };
        assert_eq!(cascade.first.comments.len(), 1);
        assert_eq!(cascade.first.comments[0].anchor, Anchor::After);
        assert!(cascade.rest[0].comments.is_empty());
    }
    assert_eq!(before.without_positions(), after.without_positions());
}
