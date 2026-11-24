//! The machine-readable tables of the specification's bytecode chapter
//! (`docs/spec/bytecode.md`), which the tests of the instructions, their
//! wire encoding and their eight-byte form hold the code to.

/// The chapter the tables stand in, as the book has it.
const BYTECODE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/spec/bytecode.md"
));

/// The rows of the fenced block whose info string is `spec-table <name>`,
/// each non-empty line split at whitespace; panics when the chapter has no
/// such block.
pub(crate) fn bytecode_table(name: &str) -> Vec<Vec<String>> {
    let opening = format!("```spec-table {name}");
    let mut lines = BYTECODE
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .skip_while(|line| line.trim() != opening);
    assert!(
        lines.next().is_some(),
        "the bytecode chapter has no table `{name}`"
    );
    lines
        .take_while(|line| line.trim() != "```")
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split_whitespace().map(str::to_string).collect())
        .collect()
}
