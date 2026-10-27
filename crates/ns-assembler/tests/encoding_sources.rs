//! Every encoding constant of an encoder is a named `const` documented with
//! its official source, so a reader can check a byte against the
//! architecture's manual instead of trusting it, and an encoder added later
//! is held to the same rule (`CLAUDE.md`, "Conventions").

use std::path::Path;

/// What an encoder file's constant must cite, by the file's name.
fn required_citation(file_name: &str) -> Option<&'static str> {
    match file_name {
        "x64.rs" => Some("Intel SDM Vol. 2"),
        "aarch64.rs" => Some("https://developer.arm.com/documentation/"),
        _ => None,
    }
}

/// The code of a line: its text without a trailing `//` comment and without
/// the contents of string literals, so a literal in either is not counted.
fn code_of(line: &str) -> String {
    let mut code = String::new();
    let mut in_string = false;
    let mut previous = ' ';
    for character in line.chars() {
        if in_string {
            if character == '"' && previous != '\\' {
                in_string = false;
            }
        } else if character == '"' {
            in_string = true;
        } else if character == '/' && previous == '/' {
            code.pop();
            break;
        } else {
            code.push(character);
        }
        previous = character;
    }
    code
}

/// Whether `code` holds a hexadecimal or binary integer literal.
fn has_pattern_literal(code: &str) -> bool {
    let characters: Vec<char> = code.chars().collect();
    characters.windows(3).enumerate().any(|(index, window)| {
        let starts_literal = window[0] == '0'
            && match window[1] {
                'x' => window[2].is_ascii_hexdigit(),
                'b' => matches!(window[2], '0' | '1'),
                _ => false,
            };
        let alone = index == 0
            || !(characters[index - 1].is_alphanumeric() || characters[index - 1] == '_');
        starts_literal && alone
    })
}

fn starts_const_item(code: &str) -> bool {
    let trimmed = code.trim_start();
    let rest = trimmed
        .strip_prefix("pub(crate) ")
        .or_else(|| trimmed.strip_prefix("pub "))
        .unwrap_or(trimmed);
    rest.starts_with("const ") && !rest.starts_with("const fn ")
}

/// Every violation of the rule in `source`, as `file:line: what`.
fn violations(file_name: &str, source: &str) -> Vec<String> {
    let citation = required_citation(file_name)
        .unwrap_or_else(|| panic!("{file_name} is an encoder file with no citation rule"));
    let lines: Vec<&str> = source.lines().collect();
    let mut found = Vec::new();
    let mut inside_const: Option<usize> = None;
    let mut const_has_literal = false;
    for (index, line) in lines.iter().enumerate() {
        let code = code_of(line);
        if inside_const.is_none() && starts_const_item(&code) {
            inside_const = Some(index);
            const_has_literal = false;
        }
        if has_pattern_literal(&code) {
            if inside_const.is_some() {
                const_has_literal = true;
            } else {
                found.push(format!(
                    "{file_name}:{}: a hexadecimal or binary literal outside a const: {}",
                    index + 1,
                    line.trim()
                ));
            }
        }
        if let Some(start) = inside_const {
            if code.trim_end().ends_with(';') {
                inside_const = None;
                if const_has_literal && !documented_with(&lines, start, citation) {
                    found.push(format!(
                        "{file_name}:{}: a const with an encoding literal and no `///` \
                         comment citing `{citation}`: {}",
                        start + 1,
                        lines[start].trim()
                    ));
                }
            }
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

#[test]
fn every_encoding_constant_is_named_and_sourced() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&directory).expect("the crate's src directory") {
        let path = entry.expect("a directory entry").path();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a UTF-8 file name")
            .to_owned();
        if !file_name.ends_with(".rs") || file_name == "lib.rs" {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("a readable source file");
        found.extend(violations(&file_name, &source));
    }
    assert!(
        found.is_empty(),
        "encoding constants must be named and sourced:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_checker_reports_a_literal_in_a_body_and_an_unsourced_const() {
    let source = "\
/// The opcode of a thing.
const THING: u8 = 0x90;

/// Intel SDM Vol. 2B, NOP—No Operation.
const SOURCED: [u8; 2] = [
    0x0F,
    0x1F,
];

fn body(code: &mut Vec<u8>) {
    code.push(0xC3); // 0x90 in a comment is no literal
    code.push(0b11 << 6);
    code.push(SOURCED[0]);
}
";
    let found = violations("x64.rs", source);
    assert_eq!(found.len(), 3, "{found:#?}");
    assert!(found[0].starts_with("x64.rs:2:") && found[0].contains("no `///` comment citing"));
    assert!(found[1].starts_with("x64.rs:11:") && found[1].contains("outside a const"));
    assert!(found[2].starts_with("x64.rs:12:") && found[2].contains("outside a const"));
}

#[test]
fn an_arm_constant_cites_a_developer_arm_com_page() {
    let sourced = "\
/// `MOVZ` <https://developer.arm.com/documentation/ddi0602/latest/Base-Instructions/MOVZ>
const MOVE_WIDE_ZERO: u32 = 0x5280_0000;
";
    assert!(violations("aarch64.rs", sourced).is_empty());
    let unsourced = sourced.replace("developer.arm.com", "example.com");
    assert_eq!(violations("aarch64.rs", &unsourced).len(), 1);
}
