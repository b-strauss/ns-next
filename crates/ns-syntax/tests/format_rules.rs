//! The style rules of `docs/design/formatting.md`, one test per rule the
//! printer of declarations decides.

use indoc::indoc;
use ns_syntax::LineEnding;

fn formatted(source: &str) -> String {
    match ns_syntax::format(source) {
        Ok(text) => text,
        Err(diagnostics) => panic!("{diagnostics:?}"),
    }
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
        class Combined = SomeRatherLongSuperclassName
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
}

#[test]
fn rule_23_the_first_line_ending_decides_every_line_ending() {
    let input = "class A = () (\r\n public one = ( ^1 )\n)";
    assert_eq!(
        formatted(input),
        "class A = () (\r\n  public one = (\r\n    ^1.\r\n  )\r\n)\r\n"
    );
    assert_eq!(LineEnding::of("a\rb\r\n"), LineEnding::Cr);
    assert_eq!(LineEnding::of("a\r\nb\n"), LineEnding::CrLf);
    assert_eq!(LineEnding::of("a\nb\r\n"), LineEnding::Lf);
    assert_eq!(LineEnding::of("a"), LineEnding::Lf);
    assert_eq!(LineEnding::Cr.as_str(), "\r");
}

#[test]
fn print_module_ends_with_one_line_ending() {
    let Ok(module) = ns_syntax::parse("class A = () ()\n\n\n") else {
        panic!("parses");
    };
    assert_eq!(
        ns_syntax::print_module(&module, LineEnding::CrLf),
        "class A = () ()\r\n"
    );
}
