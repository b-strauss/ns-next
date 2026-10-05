//! Splits a script of `newspeak.json` into the arguments `main:args:` is
//! handed, as a POSIX shell splits a command line into words.
//!
//! The rules are a POSIX shell's on every platform, rather than each
//! platform's own (a Windows program's command line is split otherwise), so
//! that one `newspeak.json` gives the same arguments on every system:
//!
//! - unquoted whitespace -- a space, a tab or a newline -- separates
//!   arguments, and whitespace at either end or repeated yields no empty
//!   argument; any other character, a carriage return or a no-break space
//!   included, belongs to an argument;
//! - `'…'` holds everything literally up to the next `'`;
//! - `"…"` holds everything literally except a backslash before `"`, `\`,
//!   `$`, a backtick or a newline, which escapes that character; a
//!   backslash before a newline removes both, and any other backslash stays;
//! - outside quotes a backslash escapes the next character, and a
//!   backslash before a newline removes both;
//! - adjacent parts make one argument (`a"b c"'d'` is `ab cd`), and `""` or
//!   `''` alone is one empty argument.
//!
//! Nothing is expanded: `$`, `*`, `?`, `~` and backticks are literal. There
//! are no operators and no comments either: `|`, `<`, `>`, `;`, `&`, `(`
//! and `)` are plain characters, and so is `#`, even at the start of a word,
//! where a shell would begin a comment. A script is handed to `main:args:`
//! and no shell runs it. An unterminated quote and a backslash at the very
//! end are refused ([`split_command_line`]).

/// The arguments `text` holds by the rules above, or why it holds none: an
/// unterminated `'` or `"`, or a backslash ending it.
pub(crate) fn split_command_line(text: &str) -> Result<Vec<String>, String> {
    let mut arguments = Vec::new();
    // `Some` once an argument has begun, even with nothing in it yet, so
    // that `""` alone is one empty argument while whitespace makes none.
    let mut argument: Option<String> = None;
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        match character {
            ' ' | '\t' | '\n' => arguments.extend(argument.take()),
            '\'' => {
                let argument = argument.get_or_insert_with(String::new);
                loop {
                    match characters.next() {
                        Some('\'') => break,
                        Some(quoted) => argument.push(quoted),
                        None => return Err("an unterminated `'`".to_string()),
                    }
                }
            }
            '"' => {
                let argument = argument.get_or_insert_with(String::new);
                loop {
                    match characters.next() {
                        Some('"') => break,
                        Some('\\') => match characters.next() {
                            Some(escaped @ ('"' | '\\' | '$' | '`')) => argument.push(escaped),
                            Some('\n') => {}
                            Some(other) => {
                                argument.push('\\');
                                argument.push(other);
                            }
                            None => return Err("an unterminated `\"`".to_string()),
                        },
                        Some(quoted) => argument.push(quoted),
                        None => return Err("an unterminated `\"`".to_string()),
                    }
                }
            }
            // A backslash before a newline joins two lines, so it neither
            // begins an argument nor ends one.
            '\\' => match characters.next() {
                Some('\n') => {}
                Some(escaped) => argument.get_or_insert_with(String::new).push(escaped),
                None => return Err("a backslash at its end, escaping nothing".to_string()),
            },
            other => argument.get_or_insert_with(String::new).push(other),
        }
    }
    arguments.extend(argument);
    Ok(arguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(text: &str) -> Vec<String> {
        split_command_line(text).unwrap_or_else(|error| panic!("{text:?}: {error}"))
    }

    fn refusal(text: &str) -> String {
        match split_command_line(text) {
            Ok(arguments) => panic!("{text:?} was split into {arguments:?}"),
            Err(error) => error,
        }
    }

    #[test]
    fn plain_words_are_separate_arguments() {
        assert_eq!(split("--timed Sieve 3"), ["--timed", "Sieve", "3"]);
    }

    #[test]
    fn runs_of_whitespace_and_whitespace_at_the_ends_yield_no_empty_argument() {
        assert_eq!(split("  a \t\n b\t"), ["a", "b"]);
        assert!(split("").is_empty());
        assert!(split(" \t\n ").is_empty());
    }

    #[test]
    fn whitespace_other_than_space_tab_and_newline_belongs_to_an_argument() {
        assert_eq!(split("a\rb c\u{a0}d"), ["a\rb", "c\u{a0}d"]);
    }

    #[test]
    fn single_quotes_hold_everything_literally() {
        assert_eq!(
            split(r#"'second word' 'a\b "c" $HOME'"#),
            ["second word", r#"a\b "c" $HOME"#]
        );
        assert_eq!(split("'line\none'"), ["line\none"]);
        assert_eq!(split(r"'b\' c"), [r"b\", "c"]);
    }

    #[test]
    fn double_quotes_hold_everything_but_their_escapes_literally() {
        assert_eq!(split(r#""second word""#), ["second word"]);
        assert_eq!(split(r#""a\"b""#), [r#"a"b"#]);
        assert_eq!(split(r#""a\\b""#), [r"a\b"]);
        assert_eq!(split(r#""a\$b""#), ["a$b"]);
        assert_eq!(split(r#""a\`b""#), ["a`b"]);
        assert_eq!(split("\"a\\\nb\""), ["ab"]);
        assert_eq!(split(r#""a\nb\x""#), [r"a\nb\x"]);
        assert_eq!(split(r#""it's""#), ["it's"]);
    }

    #[test]
    fn outside_quotes_a_backslash_escapes_the_next_character() {
        assert_eq!(split(r"second\ word"), ["second word"]);
        assert_eq!(split(r#"\"quoted\" \'"#), [r#""quoted""#, "'"]);
        assert_eq!(split(r"a\\b \n"), [r"a\b", "n"]);
        assert_eq!(split("a\\\nb"), ["ab"]);
        assert_eq!(split("a \\\n b"), ["a", "b"]);
    }

    #[test]
    fn adjacent_parts_make_one_argument() {
        assert_eq!(split(r#"a"b c"'d'"#), ["ab cd"]);
        assert_eq!(split(r#"--name="Delta Blue""#), ["--name=Delta Blue"]);
    }

    #[test]
    fn empty_quotes_alone_are_one_empty_argument() {
        assert_eq!(split(r#""" '' x"#), ["", "", "x"]);
        assert_eq!(split(r#"a"" ''b"#), ["a", "b"]);
    }

    #[test]
    fn nothing_is_expanded() {
        assert_eq!(
            split("$HOME * ~ `date` a|b c>d e;f ?"),
            ["$HOME", "*", "~", "`date`", "a|b", "c>d", "e;f", "?"]
        );
        assert_eq!(split(r#""$HOME" "*""#), ["$HOME", "*"]);
    }

    #[test]
    fn a_hash_parentheses_and_an_ampersand_are_plain_characters() {
        assert_eq!(split("a #b (c) & d"), ["a", "#b", "(c)", "&", "d"]);
    }

    #[test]
    fn an_unterminated_single_quote_is_refused() {
        let error = refusal("'unterminated");
        assert!(error.contains("unterminated `'`"), "{error}");
        assert!(refusal(r"a 'b\'c'").contains("unterminated `'`"));
    }

    #[test]
    fn an_unterminated_double_quote_is_refused() {
        let error = refusal(r#"a "b"#);
        assert!(error.contains(r#"unterminated `"`"#), "{error}");
        assert!(refusal(r#""b\""#).contains(r#"unterminated `"`"#));
        assert!(refusal(r#""b\"#).contains(r#"unterminated `"`"#));
    }

    #[test]
    fn a_backslash_at_the_very_end_is_refused() {
        let error = refusal(r"a b\");
        assert!(error.contains("backslash at its end"), "{error}");
    }
}
