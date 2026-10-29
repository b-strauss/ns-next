//! The JIT's architecture split (`docs/design/execution.md`, "The
//! foundation and the one extension point", "Fixed registers and the
//! calling convention"): a template, a send sequence or a stub is written
//! against the macro assembler's idioms and its `Roles`, never against an
//! encoder, so which machine register plays which role is decided in one
//! file per architecture, and a second architecture is that file's.
//!
//! The rule, for every file under `src/jit/` but `macro_assembler.rs`:
//! no code outside a test module (`#[cfg(test)]`, or an `all` requiring
//! `test`) names `ns_assembler`'s `x64` module -- a `use` of it, a path
//! through it. There is no exception: even the stack pointer the stubs
//! save and reset is a role (`Roles::stack_pointer`), since x64's `RSP`
//! is AArch64's `sp`.

use std::path::{Path, PathBuf};

/// The file that decides the registers.
const DECIDING_FILE: &str = "macro_assembler.rs";

/// Every `.rs` file under `directory`, subdirectories included.
fn sources(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let entries = std::fs::read_dir(directory).expect("the JIT's sources are readable");
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            found.extend(sources(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// A line's code: its text before a `//` comment, which a register's
/// name in prose may stand in.
fn code_of(line: &str) -> &str {
    line.find("//").map_or(line, |start| &line[..start])
}

/// Whether `code` names the identifier `x64` on its own, as a module path
/// segment does, rather than as part of a longer identifier.
fn names_the_x64_module(code: &str) -> bool {
    let is_identifier = |character: char| character.is_ascii_alphanumeric() || character == '_';
    code.match_indices("x64").any(|(start, _)| {
        let before = code[..start].chars().next_back();
        let after = code[start + 3..].chars().next();
        !before.is_some_and(is_identifier) && !after.is_some_and(is_identifier)
    })
}

/// Whether `line` is a `cfg` attribute that holds its item to test builds
/// only: `#[cfg(test)]`, or an `all(...)` one of whose own operands is
/// `test`. A `test` nested deeper -- `all(any(test, debug_assertions),
/// ...)` -- does not count, since such an item is compiled outside tests.
fn is_test_configuration(line: &str) -> bool {
    let line = line.trim();
    if line == "#[cfg(test)]" {
        return true;
    }
    let Some(operands) = line
        .strip_prefix("#[cfg(all(")
        .and_then(|rest| rest.strip_suffix("))]"))
    else {
        return false;
    };
    top_level_operands(operands)
        .iter()
        .any(|operand| operand.trim() == "test")
}

/// The comma-separated operands of a `cfg` predicate's argument list, a
/// comma inside a nested `any(...)`, `not(...)` or `all(...)` not
/// splitting them.
fn top_level_operands(operands: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let (mut depth, mut start) = (0_usize, 0);
    for (index, character) in operands.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                found.push(&operands[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    found.push(&operands[start..]);
    found
}

/// The lines of `source` outside its `#[cfg(test)]` modules, each with its
/// line number: a `mod` item right after the attribute is skipped to the
/// brace that closes it.
fn lines_outside_test_modules(source: &str) -> Vec<(usize, &str)> {
    let mut kept = Vec::new();
    let mut lines = source.lines().enumerate().peekable();
    while let Some((number, line)) = lines.next() {
        if !is_test_configuration(line) {
            kept.push((number + 1, line));
            continue;
        }
        let opens_a_module = lines.peek().is_some_and(|(_, next)| {
            next.trim_start().starts_with("mod ") && next.trim_end().ends_with('{')
        });
        if !opens_a_module {
            kept.push((number + 1, line));
            continue;
        }
        let mut depth = 0_i64;
        for (_, module_line) in lines.by_ref() {
            let code = code_of(module_line);
            depth += code.matches('{').count() as i64;
            depth -= code.matches('}').count() as i64;
            if depth <= 0 {
                break;
            }
        }
    }
    kept
}

#[test]
fn no_jit_file_but_the_macro_assembler_names_an_x64_register() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("jit");
    let mut violations = Vec::new();
    let mut checked = 0;
    for path in sources(&directory) {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a source's name is text");
        if name == DECIDING_FILE {
            continue;
        }
        checked += 1;
        let source = std::fs::read_to_string(&path).expect("a JIT source is readable");
        for (number, line) in lines_outside_test_modules(&source) {
            let code = code_of(line);
            if !names_the_x64_module(code) {
                continue;
            }
            violations.push(format!("{}:{number}: {}", path.display(), line.trim()));
        }
    }
    assert!(
        checked >= 5,
        "the scan reached the JIT's sources: {checked} files"
    );
    assert!(
        violations.is_empty(),
        "a register is chosen outside `{DECIDING_FILE}`; name it as a role in `Roles` \
         instead:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_scan_finds_a_use_and_a_path_but_not_prose_or_a_test_module() {
    assert!(names_the_x64_module("use ns_assembler::x64::{RAX, RCX};"));
    assert!(names_the_x64_module(
        "    let value = ns_assembler::x64::RAX;"
    ));
    assert!(!names_the_x64_module(code_of("    // x64's registers")));
    assert!(!names_the_x64_module("fn the_jit_is_available_on_x64() {"));
    assert!(!names_the_x64_module("Assembler<X64>"));
    assert!(is_test_configuration(
        "#[cfg(all(test, target_arch = \"x86_64\"))]"
    ));
    assert!(!is_test_configuration("#[cfg(debug_assertions)]"));
    assert!(!is_test_configuration(
        "#[cfg(all(any(test, debug_assertions), target_arch = \"x86_64\"))]"
    ));
    assert!(!is_test_configuration("#[cfg(all(not(test), unix))]"));
    assert!(!is_test_configuration("#[cfg(any(test, unix))]"));
    let source = "use a::b;\n#[cfg(test)]\nmod tests {\n    use ns_assembler::x64::RAX;\n    fn f() {\n    }\n}\nuse ns_assembler::x64::RCX;\n";
    let kept: Vec<usize> = lines_outside_test_modules(source)
        .into_iter()
        .filter(|(_, line)| names_the_x64_module(code_of(line)))
        .map(|(number, _)| number)
        .collect();
    assert_eq!(kept, vec![8]);
}
