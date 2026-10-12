//! Every Newspeak source of the repository outside `corpus/` and
//! `prior-art/` is written exactly as `ns-next-analyzer fmt` writes it.

/// The directories the gate holds, relative to the repository's root.
const DIRECTORIES: [&str; 3] = ["goldens", "benchmarks", "crates/ns-runtime/core-library"];

#[test]
fn every_repository_source_is_formatted() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for directory in DIRECTORIES {
        ns_tooling::source::collect_ns_files(&root.join(directory), &mut files);
    }
    files.sort();
    assert!(
        files.len() > 380,
        "the gate found only {} sources",
        files.len()
    );
    let mut unformatted = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).expect("a readable source");
        let name = relative_name(&root, file);
        match ns_syntax::format(&source) {
            Ok(formatted) if formatted == source => {}
            Ok(formatted) => unformatted.push(format!(
                "{name}: first difference at line {}",
                first_differing_line(&source, &formatted)
            )),
            Err(_) => unformatted.push(format!("{name}: does not parse")),
        }
    }
    // Run at the root, `fmt` would rewrite `corpus/` and `prior-art/` too.
    assert!(
        unformatted.is_empty(),
        "run `cargo run -p ns-analyzer -- fmt {}`:\n{}",
        DIRECTORIES.join(" "),
        unformatted.join("\n")
    );
}

/// `file` relative to `root`, its directories separated by `/` on every
/// platform.
fn relative_name(root: &std::path::Path, file: &std::path::Path) -> String {
    let relative = file.strip_prefix(root).unwrap_or(file);
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// The number, from 1, of the first line that differs between `left` and
/// `right`, its line ending included: `\r\n`, `\r` and `\n` each end one.
fn first_differing_line(left: &str, right: &str) -> usize {
    let left = lines_with_their_endings(left);
    let right = lines_with_their_endings(right);
    left.iter()
        .zip(&right)
        .position(|(first, second)| first != second)
        .unwrap_or_else(|| left.len().min(right.len()))
        + 1
}

/// `text`'s lines, each with the line ending that ends it.
fn lines_with_their_endings(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let end = match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => index + 2,
            b'\r' | b'\n' => index + 1,
            _ => {
                index += 1;
                continue;
            }
        };
        lines.push(&text[start..end]);
        start = end;
        index = end;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

#[test]
fn the_first_differing_line_counts_every_line_ending() {
    assert_eq!(first_differing_line("a\nb\nc\n", "a\nb\nd\n"), 3);
    assert_eq!(first_differing_line("a\r\nb\n", "a\nb\n"), 1);
    assert_eq!(first_differing_line("a\rb\rc", "a\rb\rd"), 3);
    assert_eq!(first_differing_line("a\nb\n", "a\nb\nc\n"), 3);
}
