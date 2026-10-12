//! The style rules of `docs/design/formatting.md`, one test per rule the
//! printer decides, for declarations, expressions and comments.

use indoc::indoc;
use ns_syntax::LineEnding;

fn formatted(source: &str) -> String {
    match ns_syntax::format(source) {
        Ok(text) => text,
        Err(diagnostics) => panic!("{diagnostics:?}"),
    }
}

/// `statements` as the body of a method `m` of a class `A`, unformatted.
fn in_method(statements: &str) -> String {
    format!("class A = () (\npublic m = (\n{statements}\n)\n)\n")
}

/// What `in_method` formats to when the body formats to `lines` (each line
/// ending with a line break, indented from column 0).
fn method_text(lines: &str) -> String {
    let body: String = lines
        .lines()
        .map(|line| {
            if line.is_empty() {
                "\n".to_string()
            } else {
                format!("    {line}\n")
            }
        })
        .collect();
    format!("class A = () (\n  public m = (\n{body}  )\n)\n")
}

#[test]
fn rule_12_a_long_binary_chain_breaks_before_each_operator() {
    let input = in_method(
        "message:: 'expected ' , expected printString , ' but got ' , actual printString , ' instead of it'",
    );
    let expected = method_text(indoc! {"
        message:: 'expected '
            , expected printString
            , ' but got '
            , actual printString
            , ' instead of it'.
    "});
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_13_block_arguments_hug_their_keywords() {
    let input =
        in_method("count > limit ifTrue: [ self reset. ^nil ] ifFalse: [ count:: count + 1 ]");
    let expected = method_text(indoc! {"
        count > limit ifTrue: [
          self reset.
          ^nil.
        ] ifFalse: [
          count:: count + 1.
        ].
    "});
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_13_a_keyword_message_that_does_not_fit_breaks_after_the_first_keyword() {
    let input = in_method(
        "result:: dictionary at: someRatherLongKeyName ifAbsent: someRatherLongDefaultValue ifPresent: transformation",
    );
    let expected = method_text(indoc! {"
        result:: dictionary at: someRatherLongKeyName
            ifAbsent: someRatherLongDefaultValue
            ifPresent: transformation.
    "});
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_14_cascades_always_break() {
    let input = in_method("stream nextPutAll: 'a'; print: 3; yourself");
    let expected = method_text(indoc! {"
        stream
            nextPutAll: 'a';
            print: 3;
            yourself.
    "});
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_15_blocks_are_tight_and_one_line_only_with_one_statement() {
    assert_eq!(
        formatted(&in_method("items do: [ :each | each reset ]")),
        method_text("items do: [:each | each reset].\n")
    );
    let expected = method_text(indoc! {"
        items do: [:each |
          each reset.
          each start.
        ].
    "});
    assert_eq!(
        formatted(&in_method("items do: [:each | each reset. each start]")),
        expected
    );
    assert_eq!(
        formatted(&in_method("x:: [ ]. y:: [ :each | ]")),
        method_text("x:: [].\ny:: [:each |].\n")
    );
}

#[test]
fn rule_15_a_block_with_temporaries_is_multi_line() {
    let expected = method_text(indoc! {"
        items do: [:each |
          |
            sum
          |
          sum:: each * 2.
          total:: total + sum.
        ].
    "});
    assert_eq!(
        formatted(&in_method(
            "items do: [:each | | sum | sum:: each * 2. total:: total + sum]"
        )),
        expected
    );
}

#[test]
fn rule_15_a_receiver_block_opens_at_the_line_end() {
    let expected = method_text(indoc! {"
        [
          self step.
          self isDone not.
        ] whileTrue.
    "});
    assert_eq!(
        formatted(&in_method("[self step. self isDone not] whileTrue")),
        expected
    );
}

#[test]
fn rule_16_parentheses_are_kept() {
    assert_eq!(
        formatted(&in_method("^( ( a foo ) bar )")),
        method_text("^((a foo) bar).\n")
    );
}

#[test]
fn rule_17_tuples_are_blocks_of_elements() {
    assert_eq!(
        formatted(&in_method("x:: { 1 . 2 . 3 . }")),
        method_text("x:: {1. 2. 3}.\n")
    );
    let expected = method_text(indoc! {"
        expected:: {
          'first element of a tuple that is long enough'.
          'second element of a tuple that is long enough'.
        }.
    "});
    assert_eq!(
        formatted(&in_method(
            "expected:: {'first element of a tuple that is long enough'. 'second element of a tuple that is long enough'}"
        )),
        expected
    );
}

#[test]
fn rule_18_literals_are_kept_as_written() {
    assert_eq!(
        formatted(&in_method("^{16r1F. #'foo'. 'it''s'. \"c\". 1.5e3}")),
        method_text("^{16r1F. #'foo'. 'it''s'. \"c\". 1.5e3}.\n")
    );
}

#[test]
fn rule_18_a_multi_line_string_is_not_reindented() {
    let input = "class A = () (\n        public m = ( ^'one\n  two' )\n)\n";
    let expected = "class A = () (\n  public m = (\n    ^'one\n  two'.\n  )\n)\n";
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_19_and_25_spacing() {
    assert_eq!(
        formatted(&in_method("count::count+1. a   <-:   foo. ^ x at:1")),
        method_text("count:: count + 1.\na <-: foo.\n^x at: 1.\n")
    );
}

#[test]
fn rule_12_a_long_unary_chain_puts_each_selector_on_its_own_line() {
    let input = in_method(
        "result:: someCollectionWithAVeryLongName asSortedCollection reversed asArray asOrderedCollection asSet",
    );
    let expected = method_text(indoc! {"
        result:: someCollectionWithAVeryLongName
            asSortedCollection
            reversed
            asArray
            asOrderedCollection
            asSet.
    "});
    assert_eq!(formatted(&input), expected);
}

#[test]
fn value_site_type_arguments_follow_their_receiver() {
    assert_eq!(
        formatted(&in_method("^Box[Integer]   new")),
        method_text("^Box[Integer] new.\n")
    );
}

#[test]
fn an_implicit_receiver_prints_nothing_and_an_explicit_self_is_kept() {
    assert_eq!(
        formatted(&in_method("foo:3. self   foo. bar baz")),
        method_text("foo: 3.\nself foo.\nbar baz.\n")
    );
}

#[test]
fn rule_19_a_setter_breaks_inside_its_value_never_after_the_colons() {
    let input = in_method(
        "someRatherLongVariableName:: firstOperandWithALongName + secondOperandWithALongName + thirdOperandName",
    );
    let expected = method_text(indoc! {"
        someRatherLongVariableName:: firstOperandWithALongName
            + secondOperandWithALongName
            + thirdOperandName.
    "});
    assert_eq!(formatted(&input), expected);
}

/// Outside in: the binary level breaks first and every operand that then
/// fits stays whole; only an operand still too long breaks its own chain.
#[test]
fn rule_12_an_operand_breaks_its_unary_chain_only_after_the_binary_level() {
    let input = in_method(
        "total:: firstValueWithALongName asFloat + anotherValueWithAVeryLongName asSortedCollection asOrderedCollection asArray reversed first asFloat + thirdValueWithAnEvenLongerName asFloat",
    );
    let expected = method_text(indoc! {"
        total:: firstValueWithALongName asFloat
            + anotherValueWithAVeryLongName
                asSortedCollection
                asOrderedCollection
                asArray
                reversed
                first
                asFloat
            + thirdValueWithAnEvenLongerName asFloat.
    "});
    assert_eq!(formatted(&input), expected);
}

/// A keyword message whose first line ends at a block's `[` keeps a
/// non-block argument after it on the `]` line.
#[test]
fn rule_13_a_short_block_hugs_when_the_message_does_not_fit_on_one_line() {
    let input = in_method(
        "collection inject: initialValueWithAVeryLongName into: [:accumulated :each | accumulated + each weightedValue]",
    );
    let expected = method_text(indoc! {"
        collection inject: initialValueWithAVeryLongName into: [:accumulated :each |
          accumulated + each weightedValue.
        ].
    "});
    assert_eq!(formatted(&input), expected);
}

/// When even the line up to the first `[` does not fit, the message breaks
/// by the keyword rule and each block is one line again where it fits.
#[test]
fn rule_13_a_message_too_long_to_hug_breaks_at_every_keyword() {
    let input = in_method(
        "someReceiverWithAVeryLongNameIndeed computeSomethingFrom: anArgumentWithAnotherLongName ifAbsent: [nil] ifPresent: [:found | found]",
    );
    let expected = method_text(indoc! {"
        someReceiverWithAVeryLongNameIndeed computeSomethingFrom: anArgumentWithAnotherLongName
            ifAbsent: [nil]
            ifPresent: [:found | found].
    "});
    assert_eq!(formatted(&input), expected);
}

/// A block or a tuple whose one statement or element breaks by itself has
/// no one-line form, though that statement hugs blocks of its own.
#[test]
fn rule_15_and_17_content_that_breaks_by_itself_opens_its_block_or_tuple() {
    let expected = method_text(indoc! {"
        action:: [:each |
          each isEmpty ifTrue: [
            each reset.
            each start.
          ].
        ].
        ^{
          items.
          condition ifTrue: [
            step.
            step.
          ].
        }.
    "});
    assert_eq!(
        formatted(&in_method(
            "action:: [:each | each isEmpty ifTrue: [each reset. each start]]. ^{items. condition ifTrue: [step. step]}"
        )),
        expected
    );
}

/// A block hugged by a message inside a later keyword argument, a binary
/// operand or a cascade's message opens its body two columns in from the
/// line the hugging starts on and closes at that line's indentation.
#[test]
fn rule_13_a_block_hugged_inside_an_argument_is_indented_from_its_line() {
    let expected = method_text(indoc! {"
        target at: 1 put: (items collect: [:each |
          each foo.
          each bar.
        ]).
        mapping
            at: #first put: 1;
            at: #button put: (chatButton: [
              toggle.
              refresh.
            ]).
        ^prefix , (items collect: [:each |
          each foo.
          each bar.
        ]).
    "});
    assert_eq!(
        formatted(&in_method(
            "target at: 1 put: (items collect: [:each | each foo. each bar]). mapping at: #first put: 1; at: #button put: (chatButton: [toggle. refresh]). ^prefix , (items collect: [:each | each foo. each bar])"
        )),
        expected
    );
}

/// The messages sent to a receiver that breaks by itself are decided again
/// on its last line, outside in: the keyword level breaks before its
/// binary argument, a unary run breaks when it does not fit.
#[test]
fn rule_12_the_messages_after_a_receiver_that_breaks_break_outside_in() {
    let expected = method_text(indoc! {"
        [
          self step.
          self other.
        ] on: SomeVeryLongExceptionClassName , AnotherLongExceptionClassName
            do: handlerWithAVeryLongNameIndeedAndMore.
        (target foo: [
          step.
          step.
        ])
            firstUnaryMessageName
            secondUnaryMessageName
            thirdUnaryMessageName
            fourthUnaryMessageName
            fifthUnaryMessageName.
    "});
    assert_eq!(
        formatted(&in_method(
            "[self step. self other] on: SomeVeryLongExceptionClassName, AnotherLongExceptionClassName do: handlerWithAVeryLongNameIndeedAndMore. (target foo: [step. step]) firstUnaryMessageName secondUnaryMessageName thirdUnaryMessageName fourthUnaryMessageName fifthUnaryMessageName"
        )),
        expected
    );
}

/// What follows a hugged block's `]` is measured again: a bare argument
/// there leaves the measuring to the next message that can break.
#[test]
fn rule_13_a_message_after_a_hugged_block_is_measured_again() {
    let input = in_method(
        "(button: icon action: [toggle. refresh] size: buttonSize) toolTip: (isFocused ifTrue: ['a rather long tool tip when focused'] ifFalse: ['another rather long tool tip'])",
    );
    let expected = method_text(indoc! {"
        (button: icon action: [
          toggle.
          refresh.
        ] size: buttonSize) toolTip: (isFocused ifTrue: [
          'a rather long tool tip when focused'.
        ] ifFalse: [
          'another rather long tool tip'.
        ]).
    "});
    assert_eq!(formatted(&input), expected);
}

/// A receiver that hugs a block of its own because it does not fit ends on
/// its `]` line, and the messages sent to it are decided there: the blocks
/// of a keyword message hug after `])`, a unary message follows it.
#[test]
fn rule_13_blocks_hug_after_a_receiver_that_hugs_its_own() {
    let input = in_method(indoc! {"
        ((rightCursor > right) or: [(leftCursor <= middle) and: [lessOrEqual value: (source at: leftCursor) value: (source at: rightCursor)]]) ifTrue: [target at: targetCursor put: (source at: leftCursor). leftCursor:: leftCursor + 1] ifFalse: [target at: targetCursor put: (source at: rightCursor). rightCursor:: rightCursor + 1].
        ((rightCursor > right) or: [(leftCursor <= middle) and: [lessOrEqual value: (source at: leftCursor) value: (source at: rightCursor)]]) not
    "});
    let expected = method_text(indoc! {"
        ((rightCursor > right) or: [
          (leftCursor <= middle) and: [
            lessOrEqual value: (source at: leftCursor) value: (source at: rightCursor).
          ].
        ]) ifTrue: [
          target at: targetCursor put: (source at: leftCursor).
          leftCursor:: leftCursor + 1.
        ] ifFalse: [
          target at: targetCursor put: (source at: rightCursor).
          rightCursor:: rightCursor + 1.
        ].
        ((rightCursor > right) or: [
          (leftCursor <= middle) and: [
            lessOrEqual value: (source at: leftCursor) value: (source at: rightCursor).
          ].
        ]) not.
    "});
    assert_eq!(formatted(&input), expected);
}

/// A message follows its receiver's last line only when that line ends on
/// a block's `]` the receiver opened at a line end: after a receiver broken
/// by the keyword rule, the outer level breaks first, whether or not an
/// argument of the receiver holds a block.
#[test]
fn rule_12_a_message_follows_only_a_receiver_that_ends_on_a_broken_block() {
    let input = in_method(indoc! {"
        (someReceiver withAVeryLongSelector: argumentNumberOne and: argumentNumberTwo and: (x ifTrue: [1] ifFalse: [2]) andMore: yy) printString.
        (someReceiver withAVeryLongSelector: argumentNumberOne and: argumentNumberTwo and: argumentThree andMore: yyyyyy) printString.
        transcript show: ([array replaceFrom: 1 to: 2 with: {1} startingAt: 1] on: kernel ArgumentError do: [:error | error argument]) printString; newLine.
        transcript show: ([array replaceFrom: 1 to: 2 with: {1} startingAt: 1] on: kernel ArgumentError do: [:error | error argument. error foo]) printString; newLine
    "});
    let expected = method_text(indoc! {"
        (someReceiver withAVeryLongSelector: argumentNumberOne
            and: argumentNumberTwo
            and: (x ifTrue: [1] ifFalse: [2])
            andMore: yy)
            printString.
        (someReceiver withAVeryLongSelector: argumentNumberOne
            and: argumentNumberTwo
            and: argumentThree
            andMore: yyyyyy)
            printString.
        transcript
            show: ([array replaceFrom: 1 to: 2 with: {1} startingAt: 1] on: kernel ArgumentError
                do: [:error | error argument])
                printString;
            newLine.
        transcript
            show: ([array replaceFrom: 1 to: 2 with: {1} startingAt: 1] on: kernel ArgumentError
                do: [:error |
                  error argument.
                  error foo.
                ]) printString;
            newLine.
    "});
    assert_eq!(formatted(&input), expected);
}

/// Whether a block in the receiver breaks for width or has to break (two
/// statements), the message after it is decided by the receiver's last
/// line alone: here `yy)`, so it goes on a continuation line of its own.
/// After a broken block's `]` or a multi-line string it follows on the line.
#[test]
fn rule_12_a_receiver_that_must_break_is_followed_like_one_that_breaks_for_width() {
    let input = in_method(indoc! {"
        (someReceiver withAVeryLongSelector: argumentNumberOne and: [someOtherObject doSomethingRatherLongWith: argumentNumberTwo] andMore: yy) printString.
        (someReceiver withAVeryLongSelector: argumentNumberOne and: [someOtherObject doSomething. argumentNumberTwo] andMore: yy) printString.
        (target foo: [step. step]) bar.
        'one
        two' size
    "});
    let expected = method_text(indoc! {"
        (someReceiver withAVeryLongSelector: argumentNumberOne and: [
          someOtherObject doSomethingRatherLongWith: argumentNumberTwo.
        ] andMore: yy)
            printString.
        (someReceiver withAVeryLongSelector: argumentNumberOne and: [
          someOtherObject doSomething.
          argumentNumberTwo.
        ] andMore: yy)
            printString.
        (target foo: [
          step.
          step.
        ]) bar.
    "});
    // The string's second line stays at column 0, as written.
    let expected = expected.replace("  )\n", "    'one\ntwo' size.\n  )\n");
    assert_eq!(formatted(&input), expected);
}

/// Block arguments hug after a receiver broken by the keyword rule, as
/// after any other: only the parts outside the block bodies must fit.
#[test]
fn rule_13_blocks_hug_after_a_receiver_broken_by_the_keyword_rule() {
    let input = in_method(
        "(someReceiver withAVeryLongSelector: argumentNumberOne and: argumentNumberTwo and: argumentThree andMore: yy) ifTrue: [a foo. b bar] ifFalse: [c foo]",
    );
    let expected = method_text(indoc! {"
        (someReceiver withAVeryLongSelector: argumentNumberOne
            and: argumentNumberTwo
            and: argumentThree
            andMore: yy) ifTrue: [
          a foo.
          b bar.
        ] ifFalse: [
          c foo.
        ].
    "});
    assert_eq!(formatted(&input), expected);
}

/// A receiver block too long for one line opens at the line end even with
/// one statement, and its messages follow its `]` (rule 15), the blocks of
/// a keyword message hugging there.
#[test]
fn rule_15_messages_follow_a_receiver_block_that_does_not_fit() {
    let input = in_method(indoc! {"
        [someObject withAVeryLongSelector: argumentNumberOne and: argumentNumberTwo and: argumentThreeee] value.
        [:each | each withAVeryLongSelector: argumentNumberOne and: argumentNumberTwo and: argumentThree] value: 3 value: [a foo. b bar]
    "});
    let expected = method_text(indoc! {"
        [
          someObject withAVeryLongSelector: argumentNumberOne
              and: argumentNumberTwo
              and: argumentThreeee.
        ] value.
        [:each |
          each withAVeryLongSelector: argumentNumberOne and: argumentNumberTwo and: argumentThree.
        ] value: 3 value: [
          a foo.
          b bar.
        ].
    "});
    assert_eq!(formatted(&input), expected);
}

/// Every alternative of a message with block arguments holds the blocks'
/// bodies; nesting such messages must not multiply them.
#[test]
fn deeply_nested_block_arguments_format_in_linear_time() {
    const LEVELS: usize = 30;
    let mut source = "step. step".to_string();
    for _ in 0..LEVELS {
        source = format!("condition ifTrue: [step. {source}]");
    }
    let mut lines = String::new();
    for level in 0..LEVELS {
        lines.push_str(&format!("{}condition ifTrue: [\n", "  ".repeat(level)));
        lines.push_str(&format!("{}step.\n", "  ".repeat(level + 1)));
    }
    lines.push_str(&format!("{}step.\n", "  ".repeat(LEVELS)));
    lines.push_str(&format!("{}step.\n", "  ".repeat(LEVELS)));
    for level in (0..LEVELS).rev() {
        lines.push_str(&format!("{}].\n", "  ".repeat(level)));
    }
    assert_eq!(formatted(&in_method(&source)), method_text(&lines));
}

/// A chain is lowered and laid out link by link: 100,000 links of a binary
/// and of a unary chain format on a 256 KiB stack.
#[test]
fn a_long_chain_formats_on_a_small_stack() {
    const LINKS: usize = 100_000;
    const STACK_BYTES: usize = 256 * 1024;
    std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(|| {
            for (link, continuation) in [(" + 1", "+ 1"), (" next", "next")] {
                let source = in_method(&format!("^1{}", link.repeat(LINKS)));
                let mut lines = String::from("^1\n");
                for _ in 1..LINKS {
                    lines.push_str(&format!("    {continuation}\n"));
                }
                lines.push_str(&format!("    {continuation}.\n"));
                assert!(formatted(&source) == method_text(&lines));
            }
        })
        .expect("spawn the formatting thread")
        .join()
        .expect("a long chain formats on a small stack");
}

#[test]
fn rule_2_the_prologue_as_written() {
    assert_eq!(
        formatted("Newspeak3 'Root' class A = () ()"),
        "Newspeak3\n'Root'\nclass A = () ()\n"
    );
    assert_eq!(formatted("class A = () ()"), "class A = () ()\n");
}

#[test]
fn rule_3_a_long_header_breaks_before_equals_then_at_every_keyword() {
    let input = "class NewspeakPrettyPrinter usingPlatform: platform ast: astModule parser: commonParser metadata: mdModule = Object new ()()";
    let expected = indoc! {"
        class NewspeakPrettyPrinter
            usingPlatform: platform
            ast: astModule
            parser: commonParser
            metadata: mdModule
            = Object new () ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_3_a_header_that_fits_after_breaking_before_equals() {
    let input = "class LongEnoughName usingPlatform: platform ast: astModule parser: commonParser metadata: mdModule = Object new ()()";
    // the pattern alone is 99 columns, the whole line 118
    let expected = indoc! {"
        class LongEnoughName usingPlatform: platform ast: astModule parser: commonParser metadata: mdModule
            = Object new () ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_3_a_header_that_fits_stays_on_one_line() {
    let input = "class Parser   usingPlatform:platform = Object new ( ) ( )";
    assert_eq!(
        formatted(input),
        "class Parser usingPlatform: platform = Object new () ()\n"
    );
}

#[test]
fn rule_5_slots_are_one_per_line_between_bars() {
    let input = "class Counter = ( | private count ::= 0. public step = 1. | ) ()";
    let expected = indoc! {"
        class Counter = (
          |
            private count ::= 0.
            public step = 1.
          |
        ) ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_5_simultaneous_slots_keep_their_double_bars() {
    let input = "class Pair = ( || first = 1. second ||) ()";
    let expected = indoc! {"
        class Pair = (
          ||
            first = 1.
            second.
          ||
        ) ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_6_header_initializer_expressions_are_one_per_line_with_periods() {
    let input = "class Counter = ( | count ::= 0. | count:: 1. self reset: count ) ()";
    let expected = indoc! {"
        class Counter = (
          |
            count ::= 0.
          |
          count:: 1.
          self reset: count.
        ) ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_7_members_keep_their_order_with_one_blank_line_between() {
    let input = "class A = () ( public b = ( ^2 ) public a = ( ^1 ) )";
    let expected = indoc! {"
        class A = () (
          public b = (
            ^2.
          )

          public a = (
            ^1.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_7_a_class_side_follows_the_instance_side() {
    let input = "class A = () ( public one = ( ^1 ) ) : ( public make = ( ^self new ) public two = ( ^2 ) )";
    let expected = indoc! {"
        class A = () (
          public one = (
            ^1.
          )
        ) : (
          public make = (
            ^self new.
          )

          public two = (
            ^2.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_7_a_nested_class_is_a_member_indented_one_level() {
    let input = "class Outer = () ( public first = ( ^1 ) private class Inner = ( | value = 3. | ) ( public value = ( ^value ) ) public last = ( ^2 ) )";
    let expected = indoc! {"
        class Outer = () (
          public first = (
            ^1.
          )

          private class Inner = (
            |
              value = 3.
            |
          ) (
            public value = (
              ^value.
            )
          )

          public last = (
            ^2.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_8_empty_parts() {
    assert_eq!(
        formatted("class Top = ( ) ( ) : ( )"),
        "class Top = () ()\n"
    );
    assert_eq!(formatted("class Top = ( | | ) ( )"), "class Top = () ()\n");
    assert_eq!(
        formatted("class Top = ( || || ) ( )"),
        "class Top = () ()\n"
    );
}

#[test]
fn rule_8_an_empty_header_with_members_below() {
    let input = "class Foo = ( ) ( public m = ( ) )";
    let expected = indoc! {"
        class Foo = () (
          public m = ()
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_9_a_long_method_pattern_puts_each_keyword_on_its_own_line() {
    let input = "class A = () ( public copyReplaceFrom: start to: stop with: replacement startingAt: replacementStart ifAbsent: absentBlock = ( ^start ) )";
    let expected = indoc! {"
        class A = () (
          public copyReplaceFrom: start
              to: stop
              with: replacement
              startingAt: replacementStart
              ifAbsent: absentBlock = (
            ^start.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_9_unary_and_binary_patterns() {
    let input = "class A = () ( size = ( ^0 ) protected + other = ( ^other ) )";
    let expected = indoc! {"
        class A = () (
          size = (
            ^0.
          )

          protected + other = (
            ^other.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_10_temporaries_are_always_opened_up_and_an_empty_body_is_inline() {
    let input =
        "class A = () ( public m = ( | current | current:: 0. ^current ) public noop = ( ) )";
    let expected = indoc! {"
        class A = () (
          public m = (
            |
              current
            |
            current:: 0.
            ^current.
          )

          public noop = ()
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_10_empty_temporaries_are_dropped() {
    let input = "class A = () ( public m = ( | | ^1 ) public noop = ( | | ) )";
    let expected = indoc! {"
        class A = () (
          public m = (
            ^1.
          )

          public noop = ()
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_20_types_in_patterns_and_slots() {
    let input = "class Map <[K,V]> new: capacity<Integer> = ( | size<Integer> ::= 0. | ) ( public at: key<K> ^<V|Nil> = ( ^nil ) )";
    let expected = indoc! {"
        class Map <[K, V]> new: capacity <Integer> = (
          |
            size <Integer> ::= 0.
          |
        ) (
          public at: key <K> ^<V | Nil> = (
            ^nil.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_20_two_bracket_groups_in_one_annotation() {
    let input = "class List[E] = () ( public List[E]<List[E]> = ( ^nil ) public reduce: function<[:RE def :RE|RE]> ^<Map[K,List[V]]> = ( ^nil ) )";
    let expected = indoc! {"
        class List[E] = () (
          public List[E] <List[E]> = (
            ^nil.
          )

          public reduce: function <[:RE def :RE | RE]> ^<Map[K, List[V]]> = (
            ^nil.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_20_a_where_clause_keeps_its_words_apart() {
    let input = "class R = () ( public reduce: function <[:RE :RE | RE]> ^<RE> where { C <: RE :: returnType of (receiverType) } = ( ^nil ) )";
    let expected = indoc! {"
        class R = () (
          public reduce: function <[:RE :RE | RE]> ^<RE> where { C <: RE :: returnType of (receiverType) } = (
            ^nil.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_24_a_lazy_slot_is_a_member_on_its_own_line() {
    let input = "class A = () ( lazy public cache = 3. public m = ( ^cache ) )";
    let expected = indoc! {"
        class A = () (
          lazy public cache = 3.

          public m = (
            ^cache.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_24_a_mixin_application_without_a_body_is_one_line() {
    assert_eq!(
        formatted("class Combined = Base <: Mixin ."),
        "class Combined = Base <: Mixin.\n"
    );
}

#[test]
fn rule_24_a_long_mixin_application_breaks_before_each_mixin_operator() {
    let input = "class Combined = SomeRatherLongSuperclassName <: FirstMixinWithAVeryLongName <: SecondMixinWithAnEvenLongerName.";
    let expected = indoc! {"
        class Combined
            = SomeRatherLongSuperclassName
            <: FirstMixinWithAVeryLongName
            <: SecondMixinWithAnEvenLongerName.
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_25_sends_in_superclass_clauses_and_statements() {
    let input = "class A = Base new: 1 + 2 with: #foo (| x = 'it''s' size. y = outer A. |) ( public m: arg = ( ^arg at: 1 put: 'a' , -2.5 ) )";
    let expected = indoc! {"
        class A = Base new: 1 + 2 with: #foo (
          |
            x = 'it''s' size.
            y = outer A.
          |
        ) (
          public m: arg = (
            ^arg at: 1 put: 'a' , -2.5.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn a_source_with_an_error_is_refused() {
    assert!(ns_syntax::format("class A = (").is_err());
    assert!(ns_syntax::format("class A = ( public m = ( ^").is_err());
}

#[test]
fn rule_23_the_first_line_ending_decides_every_line_ending() {
    let input = "class A = () (\r\n public one = ( ^1 )\n)";
    assert_eq!(
        formatted(input),
        "class A = () (\r\n  public one = (\r\n    ^1.\r\n  )\r\n)\r\n"
    );
    assert_eq!(LineEnding::of("a\rb\r\n"), LineEnding::CarriageReturn);
    assert_eq!(
        LineEnding::of("a\r\nb\n"),
        LineEnding::CarriageReturnLineFeed
    );
    assert_eq!(LineEnding::of("a\nb\r\n"), LineEnding::LineFeed);
    assert_eq!(LineEnding::of("a"), LineEnding::LineFeed);
    assert_eq!(LineEnding::CarriageReturn.as_str(), "\r");
}

#[test]
fn print_module_ends_with_one_line_ending() {
    let Ok(module) = ns_syntax::parse("class A = () ()\n\n\n") else {
        panic!("parses");
    };
    assert_eq!(
        ns_syntax::print_module(&module, LineEnding::CarriageReturnLineFeed),
        "class A = () ()\r\n"
    );
}

#[test]
fn rule_3_a_single_factory_keyword_goes_on_its_own_line() {
    let input = "class AVeryLongClassNameThatTakesUpQuiteSomeSpaceInTheHeader usingPlatformWithAVeryLongKeyword: platformParameterName = Object ()()";
    let expected = indoc! {"
        class AVeryLongClassNameThatTakesUpQuiteSomeSpaceInTheHeader
            usingPlatformWithAVeryLongKeyword: platformParameterName
            = Object () ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_3_a_mixin_chain_stays_on_the_equals_line_while_it_fits() {
    let input = "class Combined usingPlatform: platform ast: astModule = SomeRatherLongSuperclassName <: FirstMixin <: SecondMixin ()()";
    let expected = indoc! {"
        class Combined usingPlatform: platform ast: astModule
            = SomeRatherLongSuperclassName <: FirstMixin <: SecondMixin () ()
    "};
    assert_eq!(formatted(input), expected);
}

/// Outside in: the `<:` chain breaks before the superclass message does,
/// and that message breaks only when its own line is still too long -- a
/// continuation line of exactly 100 columns stays whole.
#[test]
fn rule_3_the_mixin_chain_breaks_before_the_superclass_message() {
    let input = indoc! {"
        class Outer = () (
        class Applied from: argument = Base withBase: (outer Outer evaluated: 'the superclass clause message here!' with: argument) <: First withFirst: (outer Outer evaluated: 'first' with: argument) () ()
        class Wider from: argument = Base withBase: (outer Outer evaluated: 'the superclass clause message here!!' with: argument) <: First withFirst: (outer Outer evaluated: 'first' with: argument) () ()
        )
    "};
    let expected = indoc! {"
        class Outer = () (
          class Applied from: argument
              = Base withBase: (outer Outer evaluated: 'the superclass clause message here!' with: argument)
              <: First withFirst: (outer Outer evaluated: 'first' with: argument) () ()

          class Wider from: argument
              = Base withBase: (outer Outer evaluated: 'the superclass clause message here!!'
                  with: argument)
              <: First withFirst: (outer Outer evaluated: 'first' with: argument) () ()
        )
    "};
    let output = formatted(input);
    assert_eq!(output, expected);
    assert_eq!(
        output.lines().nth(2).map(|line| line.chars().count()),
        Some(100)
    );
}

#[test]
fn rule_3_a_mixin_chain_too_long_for_the_equals_line_breaks_before_each_operator() {
    let input = "class Combined usingPlatform: platform = SomeRatherLongSuperclassName new <: FirstMixinWithAVeryLongName <: SecondMixinWithAnEvenLongerName ( | x = 1. | ) ()";
    let expected = indoc! {"
        class Combined usingPlatform: platform
            = SomeRatherLongSuperclassName new
            <: FirstMixinWithAVeryLongName
            <: SecondMixinWithAnEvenLongerName (
          |
            x = 1.
          |
        ) ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_3_a_factory_keeps_its_return_type_and_where_clause() {
    assert_eq!(
        formatted("class Box new: value<E> ^<Box[E]> where {E <: Object :: nil} = () ()"),
        "class Box new: value <E> ^<Box[E]> where { E <: Object :: nil } = () ()\n"
    );
    let input = "class BoxWithALongerName new: initialValueWithAVeryLongParameterName <E> ^<Box[E]> where { E <: Object :: nil } = Object new ()()";
    let expected = indoc! {"
        class BoxWithALongerName
            new: initialValueWithAVeryLongParameterName <E> ^<Box[E]> where { E <: Object :: nil }
            = Object new () ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_3_a_nested_class_header_breaks_at_its_indentation() {
    let input = "class Outer = () ( class NewspeakPrettyPrinter usingPlatform: platform ast: astModule parser: commonParser metadata: mdModule = Object new ()() )";
    let expected = indoc! {"
        class Outer = () (
          class NewspeakPrettyPrinter
              usingPlatform: platform
              ast: astModule
              parser: commonParser
              metadata: mdModule
              = Object new () ()
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_5_an_explicit_protected_slot_keeps_its_modifier() {
    let input = "class A = ( | protected count = 0. private name. | ) ()";
    let expected = indoc! {"
        class A = (
          |
            protected count = 0.
            private name.
          |
        ) ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_24_a_lazy_slot_keeps_its_modifiers_in_written_order() {
    let input = "class A = () ( public lazy first = 1. lazy private second = 2. lazy third = 3. )";
    let expected = indoc! {"
        class A = () (
          public lazy first = 1.

          lazy private second = 2.

          lazy third = 3.
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_25_an_eventual_send_and_type_arguments_as_written() {
    let input = "class A = () ( public m = ( target <-: ping. target <-: at: 1 put: 2. ^(Box)[Integer] new: Box[Integer] new ) )";
    let expected = indoc! {"
        class A = () (
          public m = (
            target <-: ping.
            target <-: at: 1 put: 2.
            ^(Box)[Integer] new: Box[Integer] new.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_21_a_line_block_is_reindented_and_its_text_kept() {
    let input = indoc! {"
        class A = () (
                ///   indented text
                /// second
                public m = ()
        )
    "};
    let expected = indoc! {"
        class A = () (
          ///   indented text
          /// second
          public m = ()
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_21_a_block_comment_moves_its_continuation_lines_with_it() {
    let input = indoc! {"
        class A = () (
                (* one
                   two *)
                public m = ()
        )
    "};
    let expected = indoc! {"
        class A = () (
          (* one
             two *)
          public m = ()
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_21_a_trailing_comment_does_not_count_toward_the_width() {
    let statement = "x:: someReceiverWithAName at: firstArgumentValue put: secondArgumentValue.";
    let input = in_method(&format!(
        "{statement} /// a remark that pushes the line well past one hundred columns"
    ));
    let expected = method_text(&format!(
        "{statement} /// a remark that pushes the line well past one hundred columns\n"
    ));
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_21_a_line_comment_inside_an_expression_breaks_its_level() {
    let input = in_method(
        "result:: dictionary at: key /// the caller guarantees presence\n ifAbsent: [nil]",
    );
    let expected = method_text(indoc! {"
        result:: dictionary at: key /// the caller guarantees presence
            ifAbsent: [nil].
    "});
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_21_an_inline_comment_stays_between_its_tokens() {
    assert_eq!(
        formatted(&in_method("self foo: (* unused *) nil bar: 3")),
        method_text("self foo: (* unused *) nil bar: 3.\n")
    );
}

#[test]
fn rule_21_an_inline_comment_after_an_opening_delimiter_has_no_space_before_it() {
    assert_eq!(
        formatted(&in_method("^( (* x *) a)")),
        method_text("^((* x *) a).\n")
    );
}

#[test]
fn rule_21_metadata_stays_between_pattern_and_equals() {
    let input = "class A = () ( public foo: x (* :exemplar: A new foo: 3 *) = ( ^x ) )";
    let expected = indoc! {"
        class A = () (
          public foo: x (* :exemplar: A new foo: 3 *) = (
            ^x.
          )
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_21_a_trailing_comment_after_the_last_statement_follows_its_period() {
    let input = in_method("x foo.\n^x bar /// done");
    let expected = method_text(indoc! {"
        x foo.
        ^x bar. /// done
    "});
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_21_a_comment_dangling_before_a_closer_stays_inside() {
    let input = in_method("^x foo.\n(* nothing more *)");
    let expected = method_text(indoc! {"
        ^x foo.
        (* nothing more *)
    "});
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_4_the_class_comment_follows_the_open_parenthesis() {
    let input = "class Foo = Object new ( (* A counter. *) | private count ::= 0. | ) ()";
    let expected = indoc! {"
        class Foo = Object new (
          (* A counter. *)
          |
            private count ::= 0.
          |
        ) ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_4_a_comment_above_the_class_stays_directly_above_it() {
    let input = "(* The module. *)\n\nclass A = () ()";
    assert_eq!(formatted(input), "(* The module. *)\nclass A = () ()\n");
}

#[test]
fn rule_7_a_members_leading_comment_sits_below_the_blank_line() {
    let input = "class A = () ( one = ()\n /// The second.\n two = () )";
    let expected = indoc! {"
        class A = () (
          one = ()

          /// The second.
          two = ()
        )
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_8_a_part_with_only_a_comment_is_opened_up() {
    assert_eq!(
        formatted("class A = () () : ( (* nothing yet *) )"),
        "class A = () () : (\n  (* nothing yet *)\n)\n"
    );
}

#[test]
fn rule_8_an_empty_slot_list_holding_a_comment_is_kept() {
    let input = "class A = ( | (* none yet *) | ) ()";
    let expected = indoc! {"
        class A = (
          |
            (* none yet *)
          |
        ) ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_23_the_first_line_ending_decides() {
    assert_eq!(
        formatted("class A = () (\r\npublic m = ( ^1 ) )"),
        "class A = () (\r\n  public m = (\r\n    ^1.\r\n  )\r\n)\r\n"
    );
}

#[test]
fn rule_23_a_string_keeps_its_own_line_breaks() {
    let input = "class A = () (\r\npublic m = ( ^'a\nb' ) )";
    let expected = "class A = () (\r\n  public m = (\r\n    ^'a\nb'.\r\n  )\r\n)\r\n";
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_23_a_string_with_carriage_returns_alone_is_multi_line() {
    // Measured as one line, the string would push `, suffix` onto a line
    // of its own; its last line leaves room for it.
    let string = format!("'{}\r{}'", "a".repeat(60), "b".repeat(60));
    let input = in_method(&format!("^{string} , suffix"));
    assert_eq!(
        formatted(&input),
        method_text(&format!("^{string} , suffix.\n"))
    );
}

#[test]
fn rule_21_a_comment_between_receiver_and_message_keeps_its_line() {
    // Leading the message, after its receiver, it stood on a line of its
    // own: on the receiver's line, it would trail the receiver.
    let input = in_method("^receiver\n(* then *) message");
    let expected = method_text(indoc! {"
        ^receiver
            (* then *)
            message.
    "});
    assert_eq!(formatted(&input), expected);
}

#[test]
fn rule_21_a_comment_ending_the_line_before_a_value_continues_it_at_four() {
    let cases = [
        (
            "x foo: /// c\n1 bar: 2",
            "x foo: /// c\n    1\n    bar: 2.\n",
        ),
        // The comment breaks the binary level it stands in (rule 21).
        ("^3 + /// c\n4", "^3\n    + /// c\n        4.\n"),
        ("y:: /// c\n1", "y:: /// c\n    1.\n"),
        ("^ /// c\n1", "^ /// c\n    1.\n"),
    ];
    for (statement, lines) in cases {
        assert_eq!(formatted(&in_method(statement)), method_text(lines));
    }
}

#[test]
fn rule_21_a_comment_ending_the_line_before_a_slot_value_continues_it_at_four() {
    let input = "class A = ( | a = /// c\n 1. | ) ()";
    let expected = indoc! {"
        class A = (
          |
            a = /// c
                1.
          |
        ) ()
    "};
    assert_eq!(formatted(input), expected);
}

#[test]
fn rule_21_two_comments_between_delimiters_are_one_space_apart() {
    assert_eq!(
        formatted(&in_method("^[ (* a *) (* b *) ]")),
        method_text("^[(* a *) (* b *)].\n")
    );
}

#[test]
fn rule_21_a_comment_between_pattern_keywords_stands_at_the_continuation() {
    let input = "class A = () ( m: x\n/// c\n n: y = () )";
    let expected = indoc! {"
        class A = () (
          m: x
              /// c
              n: y = ()
        )
    "};
    assert_eq!(formatted(input), expected);
}
