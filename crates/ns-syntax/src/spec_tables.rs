//! The machine-readable tables of the specification's lexical structure
//! (`docs/spec/language/lexical-structure.md`), which the lexer's and the
//! parser's tests hold the code to.

/// The chapter the tables stand in, as the book has it.
const LEXICAL_STRUCTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/spec/language/lexical-structure.md"
));

/// The entries of the fenced block whose info string is `spec-table
/// <name>`, split at whitespace; panics when the chapter has no such block.
pub(crate) fn lexical_table(name: &str) -> Vec<String> {
    let opening = format!("```spec-table {name}");
    let mut lines = LEXICAL_STRUCTURE
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .skip_while(|line| line.trim() != opening);
    assert!(
        lines.next().is_some(),
        "the lexical structure has no table `{name}`"
    );
    lines
        .take_while(|line| line.trim() != "```")
        .flat_map(str::split_whitespace)
        .map(str::to_string)
        .collect()
}
