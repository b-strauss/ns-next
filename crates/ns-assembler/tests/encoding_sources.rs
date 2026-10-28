//! Every encoding constant of an encoder is a named `const` documented with
//! its official source, so a reader can check a byte against the
//! architecture's manual instead of trusting it, and an encoder added later
//! is held to the same rule (`CLAUDE.md`, "Conventions").
//!
//! The rule, for every encoder file under `src/` (subdirectories included;
//! `src/lib.rs` is no encoder):
//! - every `const` item has a `///` comment directly above it that cites its
//!   source, whatever its value;
//! - no integer literal other than `0` and `1` stands outside a `const`
//!   item: an opcode, a size, a shift limit, an extension is a sourced
//!   constant.

use std::path::{Path, PathBuf};

/// What a constant of an encoder must cite, by the encoder: the file's stem
/// or, for an encoder split into a directory, the directory's name.
fn required_citation(encoder: &str) -> &'static str {
    match encoder {
        "x64" => "Intel SDM Vol. 2",
        "aarch64" => "https://developer.arm.com/documentation/",
        _ => panic!("{encoder} is an encoder with no citation rule"),
    }
}

/// How far a scan of a source has got through comments and literals.
#[derive(Clone, Copy, PartialEq)]
enum Lexical {
    Code,
    BlockComment(usize),
}

/// The code of a line: its text without comments (`//`, nested `/* */`),
/// string literals and character literals, so a number in one is not
/// counted. `state` carries a block comment across lines.
fn code_of(line: &str, state: &mut Lexical) -> String {
    let characters: Vec<char> = line.chars().collect();
    let mut code = String::new();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        let next = characters.get(index + 1).copied();
        if let Lexical::BlockComment(depth) = *state {
            if character == '*' && next == Some('/') {
                *state = if depth == 1 {
                    Lexical::Code
                } else {
                    Lexical::BlockComment(depth - 1)
                };
                index += 2;
            } else if character == '/' && next == Some('*') {
                *state = Lexical::BlockComment(depth + 1);
                index += 2;
            } else {
                index += 1;
            }
        } else if character == '/' && next == Some('/') {
            break;
        } else if character == '/' && next == Some('*') {
            *state = Lexical::BlockComment(1);
            index += 2;
        } else if character == '"' {
            index += 1;
            while index < characters.len() && characters[index] != '"' {
                index += if characters[index] == '\\' { 2 } else { 1 };
            }
            index += 1;
        } else if character == '\'' {
            // A character literal is `'x'` or `'\…'`; anything else is a
            // lifetime.
            if next == Some('\\') {
                index += 2;
                while index < characters.len() && characters[index] != '\'' {
                    index += 1;
                }
                index += 1;
            } else if characters.get(index + 2) == Some(&'\'') {
                index += 3;
            } else {
                code.push(character);
                index += 1;
            }
        } else {
            code.push(character);
            index += 1;
        }
    }
    code
}

/// What an integer literal in code is worth to the rule.
#[derive(PartialEq)]
enum Literal {
    /// `0`, `1`, in any suffix: allowed anywhere.
    ZeroOrOne,
    /// Any other literal; a hexadecimal, octal or binary one always.
    Other,
}

/// The integer literals of `code`, each as the rule judges it.
fn literals_of(code: &str) -> Vec<Literal> {
    let characters: Vec<char> = code.chars().collect();
    let mut found = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        let boundary = index == 0 || {
            let previous = characters[index - 1];
            !(previous.is_alphanumeric() || previous == '_' || previous == '.')
        };
        if character.is_ascii_digit() && boundary {
            let start = index;
            while index < characters.len()
                && (characters[index].is_alphanumeric() || characters[index] == '_')
            {
                index += 1;
            }
            let token: String = characters[start..index].iter().collect();
            let digits: String = token
                .chars()
                .take_while(|digit| digit.is_ascii_digit() || *digit == '_')
                .filter(|digit| *digit != '_')
                .collect();
            let radix_pattern = token.len() > 1
                && token.starts_with('0')
                && !digits.eq(&token)
                && { matches!(token.chars().nth(1), Some('x' | 'b' | 'o')) };
            found.push(if !radix_pattern && (digits == "0" || digits == "1") {
                Literal::ZeroOrOne
            } else {
                Literal::Other
            });
        } else {
            index += 1;
        }
    }
    found
}

fn starts_const_item(code: &str) -> bool {
    let trimmed = code.trim_start();
    let rest = trimmed
        .strip_prefix("pub(crate) ")
        .or_else(|| trimmed.strip_prefix("pub "))
        .unwrap_or(trimmed);
    rest.starts_with("const ") && !rest.starts_with("const fn ")
}

/// Every violation of the rule in `source`, a file of `encoder` at `path`,
/// as `path:line: what`.
fn violations(path: &str, encoder: &str, source: &str) -> Vec<String> {
    let citation = required_citation(encoder);
    let lines: Vec<&str> = source.lines().collect();
    let mut found = Vec::new();
    let mut state = Lexical::Code;
    let mut inside_const: Option<usize> = None;
    for (index, line) in lines.iter().enumerate() {
        let code = code_of(line, &mut state);
        if inside_const.is_none() && starts_const_item(&code) {
            inside_const = Some(index);
            if !documented_with(&lines, index, citation) {
                found.push(format!(
                    "{path}:{}: a const without a `///` comment citing `{citation}`: {}",
                    index + 1,
                    line.trim()
                ));
            }
        } else if inside_const.is_none() && literals_of(&code).contains(&Literal::Other) {
            found.push(format!(
                "{path}:{}: an integer literal other than 0 and 1 outside a const: {}",
                index + 1,
                line.trim()
            ));
        }
        if inside_const.is_some() && code.trim_end().ends_with(';') {
            inside_const = None;
        }
    }
    found
}

/// Whether the `///` lines directly above the item at `start` (attributes
/// between them skipped) contain `citation`.
fn documented_with(lines: &[&str], start: usize, citation: &str) -> bool {
    let mut documentation = String::new();
    for line in lines[..start].iter().rev() {
        let trimmed = line.trim();
        if trimmed.starts_with("///") {
            documentation.push_str(trimmed);
        } else if !(trimmed.starts_with("#[")
            || trimmed.starts_with("reason =")
            || trimmed.starts_with("clippy::")
            || trimmed.starts_with(')'))
        {
            break;
        }
    }
    documentation.contains(citation)
}

/// The encoder source files under `source_directory`, as paths relative to
/// it: every `.rs` file in it and its subdirectories but its own `lib.rs`.
fn encoder_files(source_directory: &Path) -> Vec<PathBuf> {
    fn walk(directory: &Path, root: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("a readable directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                walk(&path, root, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let relative = path.strip_prefix(root).expect("below the root").to_owned();
                if relative != Path::new("lib.rs") {
                    files.push(relative);
                }
            }
        }
    }
    let mut files = Vec::new();
    walk(source_directory, source_directory, &mut files);
    files.sort();
    files
}

/// The encoder a relative path belongs to: its first component's stem.
fn encoder_of(relative: &Path) -> String {
    let first = relative
        .components()
        .next()
        .expect("a non-empty path")
        .as_os_str()
        .to_string_lossy()
        .into_owned();
    first.strip_suffix(".rs").unwrap_or(&first).to_owned()
}

fn violations_in(source_directory: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for relative in encoder_files(source_directory) {
        let source = std::fs::read_to_string(source_directory.join(&relative))
            .expect("a readable source file");
        found.extend(violations(
            &relative.display().to_string(),
            &encoder_of(&relative),
            &source,
        ));
    }
    found
}

#[test]
fn every_encoding_constant_is_named_and_sourced() {
    let found = violations_in(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
    assert!(
        found.is_empty(),
        "encoding constants must be named and sourced:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_checker_reports_a_literal_in_a_body_and_an_uncited_const() {
    let source = "\
/// The opcode of a thing.
const THING: u8 = 0x90;

/// Intel SDM Vol. 2B, NOP—No Operation.
const SOURCED: [u8; 2] = [
    0x0F,
    0x1F,
];

/// A size, and no source.
const SIZE: i64 = 5;

fn body(code: &mut Vec<u8>) {
    code.push(0xC3); // 0x90 in a comment is no literal
    code.push(0b11 << 6);
    code.push(SOURCED[0]);
    code.push(1 + 0);
    code.truncate(4);
    /* 99 in a block
       comment, 98 too */
    let quote = '\"'; let text = \"7\";
    let count = 3u8;
}
";
    let found = violations("x64.rs", "x64", source);
    let lines: Vec<&str> = found
        .iter()
        .map(|violation| violation.split(": ").next().expect("a location"))
        .collect();
    assert_eq!(
        lines,
        [
            "x64.rs:2",
            "x64.rs:11",
            "x64.rs:14",
            "x64.rs:15",
            "x64.rs:18",
            "x64.rs:22"
        ],
        "{found:#?}"
    );
}

#[test]
fn an_arm_constant_cites_a_developer_arm_com_page() {
    let sourced = "\
/// `MOVZ` <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/MOVZ>
const MOVE_WIDE_ZERO: u32 = 0x5280_0000;
";
    assert!(violations("aarch64.rs", "aarch64", sourced).is_empty());
    let unsourced = sourced.replace("developer.arm.com", "example.com");
    assert_eq!(violations("aarch64.rs", "aarch64", &unsourced).len(), 1);
}

#[test]
fn an_encoder_split_into_a_directory_is_checked_too() {
    let root = std::env::temp_dir().join(format!("ns-assembler-sources-{}", std::process::id()));
    let directory = root.join("aarch64");
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    std::fs::write(root.join("lib.rs"), "const IGNORED: u8 = 0x12;\n").expect("lib.rs");
    std::fs::write(
        directory.join("branches.rs"),
        "const NO_SOURCE: u32 = 0x1400_0000;\n",
    )
    .expect("branches.rs");
    let found = violations_in(&root);
    std::fs::remove_dir_all(&root).expect("cleanup");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].contains("branches.rs:1"), "{found:#?}");
}
