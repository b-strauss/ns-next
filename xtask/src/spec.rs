//! The specification's book (`docs/spec/`): its rules, the tests' references
//! to them, and the HTML `cargo xtask spec` renders into `target/spec/`
//! (`docs/design/specification.md`, decisions 5 and 6).

use std::path::{Path, PathBuf};

/// A rule of the book: its identifier and where it is declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub identifier: String,
    pub chapter: PathBuf,
    pub line: usize,
}

/// A test's reference to a rule: the file, and for a Rust test the name of
/// the test function, empty for a golden, whose file is its test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub identifier: String,
    pub test: PathBuf,
    pub name: String,
    pub line: usize,
}

/// The mdBook release the book is rendered with.
pub const MDBOOK_VERSION: &str = "0.5.4";

/// The line that tells a test's reference to a rule in a Rust source.
const RUST_REFERENCE: &str = "// spec:";

/// Whether `identifier` is one: segments joined by `.`, each a lowercase
/// letter followed by lowercase letters, digits and `-`.
fn is_identifier(identifier: &str) -> bool {
    identifier.split('.').all(|segment| {
        let mut characters = segment.chars();
        characters
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
            && characters.all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            })
    })
}

/// The lines of `text` without their line ends, CRLF or LF.
fn lines_of(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
}

/// The fence a line opens or closes: its character and its length, for a
/// line that starts, after its indentation, with three or more backticks or
/// tildes (CommonMark's fenced code blocks). Any indentation is taken, so
/// that a fence inside a list item, indented to the item's text, counts.
fn fence_of(line: &str) -> Option<(char, usize)> {
    let rest = line.trim_start_matches(' ');
    let character = rest
        .chars()
        .next()
        .filter(|first| *first == '`' || *first == '~')?;
    let length = rest.chars().take_while(|next| *next == character).count();
    // A backtick fence's info string may hold no backtick, so a line that
    // opens with a code span opens no fence.
    let info = &rest[length..];
    let opens = length >= 3 && !(character == '`' && info.contains('`'));
    opens.then_some((character, length))
}

/// Calls `visit` with each line of `markdown`, its 1-based number, and
/// whether it stands outside every fenced code block; answers the line of a
/// fence still open at the end, which hides every line after it.
fn walk_markdown(markdown: &str, mut visit: impl FnMut(usize, &str, bool)) -> Option<usize> {
    let mut open: Option<(char, usize)> = None;
    let mut opened_at = 0;
    for (index, line) in lines_of(markdown).enumerate() {
        match (open, fence_of(line)) {
            (None, Some(fence)) => {
                open = Some(fence);
                opened_at = index + 1;
                visit(index + 1, line, false);
            }
            (Some((character, length)), Some((closing, closing_length)))
                if closing == character
                    && closing_length >= length
                    && line.trim().chars().all(|next| next == character) =>
            {
                open = None;
                visit(index + 1, line, false);
            }
            (Some(_), _) => visit(index + 1, line, false),
            (None, None) => visit(index + 1, line, true),
        }
    }
    open.map(|_| opened_at)
}

/// The identifier of a rule line, `r[<identifier>]` alone on its line and
/// at its start, so that an indented code block's `r[...]` is a sample.
fn rule_line(line: &str) -> Option<&str> {
    line.trim_end()
        .strip_prefix("r[")
        .and_then(|rest| rest.strip_suffix(']'))
}

/// A path as the book and its messages write it, with `/`.
fn display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Every rule `markdown` declares, read from `chapter`.
pub fn rules_in(markdown: &str, chapter: &Path) -> Result<Vec<Rule>, String> {
    let mut rules = Vec::new();
    let mut refusals = Vec::new();
    let unclosed = walk_markdown(markdown, |line, text, outside| {
        if !outside {
            return;
        }
        if text.starts_with("r[") && rule_line(text).is_none() {
            refusals.push(format!(
                "{}:{line}: a line that begins with `r[` is a rule line, `r[<identifier>]` alone",
                display(chapter)
            ));
        }
        {
            if let Some(identifier) = rule_line(text) {
                if is_identifier(identifier) {
                    rules.push(Rule {
                        identifier: identifier.to_string(),
                        chapter: chapter.to_path_buf(),
                        line,
                    });
                } else {
                    refusals.push(format!(
                        "{}:{line}: `{identifier}` is no rule identifier: segments joined by `.`, \
                         each a lowercase letter followed by lowercase letters, digits and `-`",
                        display(chapter)
                    ));
                }
            }
        }
    });
    if let Some(line) = unclosed {
        refusals.push(format!(
            "{}:{line}: this fence is never closed, which hides every rule after it",
            display(chapter)
        ));
    }
    if refusals.is_empty() {
        Ok(rules)
    } else {
        Err(refusals.join("\n"))
    }
}

/// The chapters `SUMMARY.md` links to, in its order, relative to the book.
fn summary_chapters(summary: &str) -> Vec<PathBuf> {
    let mut chapters = Vec::new();
    for line in lines_of(summary) {
        let mut rest = line;
        while let Some(start) = rest.find("](") {
            rest = &rest[start + 2..];
            if let Some(end) = rest.find(')') {
                let target = &rest[..end];
                if target.ends_with(".md") {
                    chapters.push(PathBuf::from(target));
                }
                rest = &rest[end..];
            }
        }
    }
    chapters
}

/// Every file below `directory` whose name ends in `extension`, skipping
/// `target` and hidden directories.
fn files_below(directory: &Path, extension: &str, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("{}: cannot be read: {error}", display(directory)))?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    paths.sort();
    for path in paths {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string());
        let name = name.unwrap_or_default();
        if path.is_dir() {
            if name != "target" && !name.starts_with('.') {
                files_below(&path, extension, found)?;
            }
        } else if name.ends_with(extension) {
            found.push(path);
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|error| format!("{}: cannot be read: {error}", display(path)))
}

/// The chapters of the book at `book`, in `SUMMARY.md`'s order, each
/// checked to exist, and every Markdown file of the book checked to be one.
fn book_chapters(book: &Path) -> Result<Vec<PathBuf>, String> {
    let chapters = summary_chapters(&read(&book.join("SUMMARY.md"))?);
    let mut refusals = Vec::new();
    for chapter in &chapters {
        if !book.join(chapter).is_file() {
            refusals.push(format!(
                "SUMMARY.md lists {}, which the book does not have",
                display(chapter)
            ));
        }
    }
    let mut files = Vec::new();
    files_below(book, ".md", &mut files)?;
    for file in files {
        let relative = file.strip_prefix(book).unwrap_or(&file).to_path_buf();
        if relative != Path::new("SUMMARY.md") && !chapters.contains(&relative) {
            refusals.push(format!(
                "{} is not in SUMMARY.md, so the book would leave it out",
                display(&relative)
            ));
        }
    }
    if refusals.is_empty() {
        Ok(chapters)
    } else {
        Err(refusals.join("\n"))
    }
}

/// Every rule of the book at `book`, chapter by chapter in `SUMMARY.md`'s order.
pub fn book_rules(book: &Path) -> Result<Vec<Rule>, String> {
    let mut rules: Vec<Rule> = Vec::new();
    let mut refusals = Vec::new();
    for chapter in book_chapters(book)? {
        match rules_in(&read(&book.join(&chapter))?, &chapter) {
            Ok(found) => {
                for rule in found {
                    if let Some(first) = rules
                        .iter()
                        .find(|known| known.identifier == rule.identifier)
                    {
                        refusals.push(format!(
                            "`{}` is declared twice: {}:{} and {}:{}",
                            rule.identifier,
                            display(&first.chapter),
                            first.line,
                            display(&rule.chapter),
                            rule.line
                        ));
                    } else {
                        rules.push(rule);
                    }
                }
            }
            Err(refusal) => refusals.push(refusal),
        }
    }
    if refusals.is_empty() {
        Ok(rules)
    } else {
        Err(refusals.join("\n"))
    }
}

/// `relative` taken from the directory of `chapter`, `..` resolved, as a
/// path relative to the book.
fn resolve_link(chapter: &Path, relative: &str) -> PathBuf {
    let mut parts: Vec<String> = chapter
        .parent()
        .map(|directory| {
            directory
                .components()
                .map(|component| component.as_os_str().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other.to_string()),
        }
    }
    parts.iter().collect()
}

/// Checks that every link to a chapter names one the book has, and that
/// every link to a rule, `(#r-<identifier>)` or
/// `(<chapter>#r-<identifier>)`, names a rule the chapter it points to
/// declares, so that no link of the rendered book dangles; mdBook refuses
/// neither.
pub fn check_links(book: &Path) -> Result<(), String> {
    let rules = book_rules(book)?;
    let mut refusals = Vec::new();
    for chapter in book_chapters(book)? {
        let markdown = read(&book.join(&chapter))?;
        let _ = walk_markdown(&markdown, |line, text, outside| {
            if !outside {
                return;
            }
            let mut rest = text;
            while let Some(start) = rest.find("](") {
                rest = &rest[start + 2..];
                let Some(end) = rest.find(')') else { break };
                let target = &rest[..end];
                rest = &rest[end..];
                let path = target.split('#').next().unwrap_or_default();
                if path.ends_with(".md")
                    && !path.contains("://")
                    && !book.join(resolve_link(&chapter, path)).is_file()
                {
                    refusals.push(format!(
                        "{}:{line}: the link to {path} names no chapter of the book",
                        display(&chapter)
                    ));
                    continue;
                }
                let Some((path, identifier)) = target.split_once("#r-") else {
                    continue;
                };
                let pointed = if path.is_empty() {
                    chapter.clone()
                } else {
                    resolve_link(&chapter, path)
                };
                let declared = rules
                    .iter()
                    .any(|rule| rule.identifier == identifier && rule.chapter == pointed);
                if !declared {
                    refusals.push(format!(
                        "{}:{line}: the link to `{identifier}` names no rule of {}",
                        display(&chapter),
                        display(&pointed)
                    ));
                }
            }
        });
    }
    if refusals.is_empty() {
        Ok(())
    } else {
        Err(refusals.join("\n"))
    }
}

/// The part of a golden's `.toml` the book reads: its `rules`, each with
/// where it stands. Every other key is the run's.
#[derive(serde::Deserialize)]
struct GoldenRules {
    #[serde(default)]
    rules: Vec<toml::Spanned<String>>,
}

/// The references a golden's `.toml` makes: its `rules`, a list of
/// identifiers, read as TOML, so that every spelling of the list counts,
/// each placed at the line it stands on.
fn golden_references(
    text: &str,
    test: &Path,
    references: &mut Vec<Reference>,
) -> Result<(), String> {
    let golden: GoldenRules = toml::from_str(text).map_err(|error| {
        format!(
            "{}: its `rules` are no list of the specification's rule identifiers: {error}",
            display(test)
        )
    })?;
    for rule in golden.rules {
        let line = text[..rule.span().start].matches('\n').count() + 1;
        references.push(Reference {
            identifier: rule.into_inner(),
            test: test.to_path_buf(),
            name: String::new(),
            line,
        });
    }
    Ok(())
}

/// The name of the test whose `#[test]` follows the run of `// spec:` lines
/// at `index`, or `None` when no test follows it directly.
fn test_below(lines: &[&str], index: usize) -> Option<String> {
    let mut below = index;
    while lines.get(below)?.trim().starts_with(RUST_REFERENCE) {
        below += 1;
    }
    if lines.get(below)?.trim() != "#[test]" {
        return None;
    }
    let signature = lines.get(below + 1)?.trim();
    let name = signature.strip_prefix("fn ")?.split('(').next()?;
    Some(name.to_string())
}

/// Every reference the tests below `root` make to a rule.
pub fn references_in(root: &Path) -> Result<Vec<Reference>, String> {
    let mut references = Vec::new();
    let mut refusals = Vec::new();
    let mut goldens = Vec::new();
    if root.join("goldens").is_dir() {
        files_below(&root.join("goldens"), ".toml", &mut goldens)?;
    }
    for golden in goldens {
        let relative = golden.strip_prefix(root).unwrap_or(&golden).to_path_buf();
        golden_references(&read(&golden)?, &relative, &mut references)?;
    }
    let mut sources = Vec::new();
    for directory in ["crates", "xtask"] {
        if root.join(directory).is_dir() {
            files_below(&root.join(directory), ".rs", &mut sources)?;
        }
    }
    for source in sources {
        let relative = source.strip_prefix(root).unwrap_or(&source).to_path_buf();
        let text = read(&source)?;
        let lines: Vec<&str> = lines_of(&text).collect();
        for (index, line) in lines.iter().enumerate() {
            let Some(rest) = line.trim().strip_prefix(RUST_REFERENCE) else {
                continue;
            };
            match test_below(&lines, index) {
                Some(name) => references.push(Reference {
                    identifier: rest.trim().to_string(),
                    test: relative.clone(),
                    name,
                    line: index + 1,
                }),
                None => refusals.push(format!(
                    "{}:{}: a `// spec:` line stands directly above a `#[test]`",
                    display(&relative),
                    index + 1
                )),
            }
        }
    }
    for reference in &references {
        if !is_identifier(&reference.identifier) {
            refusals.push(format!(
                "{}:{}: `{}` is no rule identifier",
                display(&reference.test),
                reference.line,
                reference.identifier
            ));
        }
    }
    if refusals.is_empty() {
        Ok(references)
    } else {
        Err(refusals.join("\n"))
    }
}

/// Checks that every reference names one of `rules`.
pub fn check_references(rules: &[Rule], references: &[Reference]) -> Result<(), String> {
    let refusals: Vec<String> = references
        .iter()
        .filter(|reference| {
            !rules
                .iter()
                .any(|rule| rule.identifier == reference.identifier)
        })
        .map(|reference| {
            format!(
                "{}:{}: `{}` names no rule of the specification",
                display(&reference.test),
                reference.line,
                reference.identifier
            )
        })
        .collect();
    if refusals.is_empty() {
        Ok(())
    } else {
        Err(refusals.join("\n"))
    }
}

/// A design document's pointer at a rule of the book: `r[<identifier>]` in
/// a code span, which the documents write so that a renamed rule breaks the
/// check instead of leaving a stale name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pointer {
    pub identifier: String,
    pub document: PathBuf,
    pub line: usize,
}

/// Every pointer `CLAUDE.md` and the design documents directly below
/// `docs/design/` make to a rule. Only a span that is a backtick, `r[`, the
/// identifier, `]` and a backtick is one, so that `` `ns-next-analyzer[.exe]` ``
/// is not; a fenced block holds samples and is skipped. A pointer is no
/// test's reference, so the rendered book lists it nowhere.
pub fn pointers_in(root: &Path) -> Result<Vec<Pointer>, String> {
    let mut documents = vec![root.join("CLAUDE.md")];
    let design = root.join("docs/design");
    if design.is_dir() {
        let mut found = Vec::new();
        let entries = std::fs::read_dir(&design)
            .map_err(|error| format!("{}: cannot be read: {error}", display(&design)))?;
        for entry in entries {
            let path = entry
                .map_err(|error| format!("{}: cannot be read: {error}", display(&design)))?
                .path();
            if path.extension().is_some_and(|extension| extension == "md") {
                found.push(path);
            }
        }
        found.sort();
        documents.extend(found);
    }
    let mut pointers = Vec::new();
    let mut refusals = Vec::new();
    for path in documents {
        if !path.is_file() {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        let text = read(&path)?;
        walk_markdown(&text, |line, content, outside| {
            if !outside {
                return;
            }
            let mut rest = content;
            while let Some(start) = rest.find("`r[") {
                let after = &rest[start + 3..];
                let Some(end) = after.find("]`") else {
                    break;
                };
                let identifier = &after[..end];
                if is_identifier(identifier) {
                    pointers.push(Pointer {
                        identifier: identifier.to_string(),
                        document: relative.clone(),
                        line,
                    });
                } else {
                    refusals.push(format!(
                        "{}:{line}: `r[{identifier}]` is no rule identifier",
                        display(&relative)
                    ));
                }
                rest = &after[end + 2..];
            }
        });
    }
    if refusals.is_empty() {
        Ok(pointers)
    } else {
        Err(refusals.join(
            "
",
        ))
    }
}

/// Checks that every pointer names one of `rules`.
pub fn check_pointers(rules: &[Rule], pointers: &[Pointer]) -> Result<(), String> {
    let refusals: Vec<String> = pointers
        .iter()
        .filter(|pointer| {
            !rules
                .iter()
                .any(|rule| rule.identifier == pointer.identifier)
        })
        .map(|pointer| {
            format!(
                "{}:{}: `r[{}]` names no rule of the specification",
                display(&pointer.document),
                pointer.line,
                pointer.identifier
            )
        })
        .collect();
    if refusals.is_empty() {
        Ok(())
    } else {
        Err(refusals.join(
            "
",
        ))
    }
}

/// `text` with the characters HTML gives a meaning escaped.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The tests that name `identifier`, each once, in order, as their file and
/// their name: a Rust test's function, or nothing for a golden, whose file
/// is the test.
fn tests_of(identifier: &str, references: &[Reference]) -> Vec<(String, String)> {
    let mut tests: Vec<(String, String)> = Vec::new();
    for reference in references {
        let test = (display(&reference.test), reference.name.clone());
        if reference.identifier == identifier && !tests.contains(&test) {
            tests.push(test);
        }
    }
    tests
}

/// The chapter that holds the VM invariants, and the only one that may
/// declare a `vm.` rule.
const VM_CHAPTER: &str = "vm-invariants.md";

/// What finds the breach of a VM invariant: a stress mode the stress gate
/// runs, a check of a debug build, or the Rust compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectorKind {
    Stress,
    Debug,
    Compiler,
}

impl DetectorKind {
    /// The kind as the table spells it.
    fn parse(cell: &str) -> Option<DetectorKind> {
        match cell {
            "stress" => Some(DetectorKind::Stress),
            "debug" => Some(DetectorKind::Debug),
            "compiler" => Some(DetectorKind::Compiler),
            _ => None,
        }
    }

    /// The kind as a rule's list of checks captions it.
    fn caption(self) -> &'static str {
        match self {
            DetectorKind::Stress => "stress mode",
            DetectorKind::Debug => "debug build",
            DetectorKind::Compiler => "Rust compiler",
        }
    }
}

/// A row of the table of detectors: what it is called, its kind, the `vm.`
/// rules whose breach it finds, and its line in the chapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detector {
    pub name: String,
    pub kind: DetectorKind,
    pub rules: Vec<String>,
    pub line: usize,
}

/// The header every table of detectors has.
const DETECTOR_HEADER: [&str; 4] = ["Detector", "Kind", "What it checks", "Rules"];

/// The cells of a Markdown table row, trimmed.
fn table_cells(line: &str) -> Vec<&str> {
    let inner = line.trim();
    let inner = inner.strip_prefix('|').unwrap_or(inner);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner.split('|').map(str::trim).collect()
}

/// The table of detectors `markdown` holds, in the one fenced block whose
/// info string is `spec-table vm-detectors`; none when it has no such block.
pub fn detectors_in(markdown: &str, chapter: &Path) -> Result<Vec<Detector>, String> {
    let location = |line: usize| format!("{}:{line}", display(chapter));
    let mut detectors = Vec::new();
    let mut refusals = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut in_table = false;
    let mut blocks = 0;
    let mut rows_seen = 0;
    let mut was_outside = true;
    let _ = walk_markdown(markdown, |line, text, outside| {
        if outside {
            was_outside = true;
            return;
        }
        if was_outside {
            was_outside = false;
            fence = fence_of(text);
            let info = text
                .trim_start_matches(' ')
                .trim_start_matches(['`', '~'])
                .trim();
            in_table = info == "spec-table vm-detectors";
            if in_table {
                blocks += 1;
                rows_seen = 0;
                if blocks == 2 {
                    refusals.push(format!(
                        "{}: a second table of detectors; the chapter has one",
                        location(line)
                    ));
                }
            }
            return;
        }
        if let (Some((character, length)), Some((closing, closing_length))) =
            (fence, fence_of(text))
            && closing == character
            && closing_length >= length
            && text.trim().chars().all(|next| next == character)
        {
            in_table = false;
            fence = None;
            return;
        }
        if !in_table || text.trim().is_empty() {
            return;
        }
        rows_seen += 1;
        let cells = table_cells(text);
        if rows_seen == 1 {
            if cells != DETECTOR_HEADER {
                refusals.push(format!(
                    "{}: the table of detectors begins with the header `| {} |`",
                    location(line),
                    DETECTOR_HEADER.join(" | ")
                ));
            }
            return;
        }
        if rows_seen == 2 {
            return;
        }
        if cells.len() != DETECTOR_HEADER.len() {
            refusals.push(format!(
                "{}: a row of the table of detectors has {} cells, not {}",
                location(line),
                cells.len(),
                DETECTOR_HEADER.len()
            ));
            return;
        }
        let Some(kind) = DetectorKind::parse(cells[1]) else {
            refusals.push(format!(
                "{}: `{}` is no kind of detector: stress, debug or compiler",
                location(line),
                cells[1]
            ));
            return;
        };
        let rules = cells[3]
            .split(',')
            .map(|cell| cell.trim().trim_matches('`').to_string())
            .filter(|cell| !cell.is_empty())
            .collect();
        detectors.push(Detector {
            name: cells[0].trim_matches('`').to_string(),
            kind,
            rules,
            line,
        });
    });
    if refusals.is_empty() {
        Ok(detectors)
    } else {
        Err(refusals.join("\n"))
    }
}

/// Holds the table of detectors to the rules and to the stress gate: every
/// rule a row names is a `vm.` rule of the book, every mode the gate runs
/// has its one `stress` row, and no `stress` row names another.
pub fn check_detectors(
    rules: &[Rule],
    detectors: &[Detector],
    stress_modes: &[&str],
) -> Result<(), String> {
    let mut refusals = Vec::new();
    for detector in detectors {
        let location = format!("{VM_CHAPTER}:{}", detector.line);
        for identifier in &detector.rules {
            if !rules.iter().any(|rule| &rule.identifier == identifier) {
                refusals.push(format!(
                    "{location}: `{identifier}` names no rule of the book"
                ));
            } else if !identifier.starts_with("vm.") {
                refusals.push(format!(
                    "{location}: `{identifier}` is no rule of the VM invariants"
                ));
            }
        }
        if detector.kind == DetectorKind::Stress && !stress_modes.contains(&detector.name.as_str())
        {
            refusals.push(format!(
                "{location}: {} is no stress mode the stress gate runs",
                detector.name
            ));
        }
    }
    for mode in stress_modes {
        let rows: Vec<&Detector> = detectors
            .iter()
            .filter(|detector| detector.kind == DetectorKind::Stress && detector.name == *mode)
            .collect();
        match rows.len() {
            0 => refusals.push(format!(
                "the stress mode {mode} has no row in the table of detectors"
            )),
            1 => {}
            _ => refusals.push(format!(
                "{VM_CHAPTER}:{}: the stress mode {mode} has more than one row",
                rows[1].line
            )),
        }
    }
    if refusals.is_empty() {
        Ok(())
    } else {
        Err(refusals.join("\n"))
    }
}

/// A rule's text as far as its chapter goes: from its line to the next rule
/// line or heading, the lines outside fenced blocks, with their numbers.
struct RuleSpan {
    identifier: String,
    line: usize,
    lines: Vec<(usize, String)>,
}

/// The spans of every rule `markdown` declares.
fn rule_spans(markdown: &str) -> Vec<RuleSpan> {
    let mut spans: Vec<RuleSpan> = Vec::new();
    let mut open = false;
    let _ = walk_markdown(markdown, |line, text, outside| {
        if !outside {
            return;
        }
        if let Some(identifier) = rule_line(text) {
            spans.push(RuleSpan {
                identifier: identifier.to_string(),
                line,
                lines: Vec::new(),
            });
            open = true;
        } else if text.starts_with('#') {
            open = false;
        } else if open && let Some(span) = spans.last_mut() {
            span.lines.push((line, text.to_string()));
        }
    });
    spans
}

/// The line that marks a rule no mechanism of the repository can detect the
/// breach of, and says why.
const UNDETECTED: &str = "Undetected:";

/// The line that names what a VM invariant protects.
const PROTECTS: &str = "Protects:";

impl RuleSpan {
    fn is_undetected(&self) -> bool {
        self.lines
            .iter()
            .any(|(_, text)| text.starts_with(UNDETECTED))
    }

    /// The identifiers the paragraph that begins with `Protects:` links to,
    /// none when the rule has no such paragraph.
    fn protected(&self) -> Option<Vec<String>> {
        let start = self
            .lines
            .iter()
            .position(|(_, text)| text.starts_with(PROTECTS))?;
        let mut protected = Vec::new();
        for (_, text) in self.lines[start..]
            .iter()
            .take_while(|(_, text)| !text.trim().is_empty())
        {
            let mut rest = text.as_str();
            while let Some(position) = rest.find("#r-") {
                rest = &rest[position + 3..];
                let end = rest.find(')').unwrap_or(rest.len());
                protected.push(rest[..end].to_string());
                rest = &rest[end..];
            }
        }
        Some(protected)
    }
}

/// The identifiers of the rules of `markdown` marked `Undetected:`.
fn undetected_in(markdown: &str) -> Vec<String> {
    rule_spans(markdown)
        .into_iter()
        .filter(RuleSpan::is_undetected)
        .map(|span| span.identifier)
        .collect()
}

/// Holds the chapter of the VM invariants to its form: no `vm.` rule is
/// declared elsewhere; each says what it protects, and through the rules it
/// protects reaches one outside the chapter; and each has a test, a
/// detector or an `Undetected:` line, the last never with either of the
/// others (`docs/design/specification.md`, "The fifth batch's decisions").
pub fn check_invariants(
    book: &Path,
    rules: &[Rule],
    references: &[Reference],
    detectors: &[Detector],
) -> Result<(), String> {
    let mut refusals = Vec::new();
    for rule in rules {
        if rule.identifier.starts_with("vm.") && rule.chapter != Path::new(VM_CHAPTER) {
            refusals.push(format!(
                "{}:{}: `{}` is declared outside the VM invariants",
                display(&rule.chapter),
                rule.line,
                rule.identifier
            ));
        }
    }
    let spans = rule_spans(&read(&book.join(VM_CHAPTER))?);
    let protected_by = |identifier: &str| -> Vec<String> {
        spans
            .iter()
            .find(|span| span.identifier == identifier)
            .and_then(RuleSpan::protected)
            .unwrap_or_default()
    };
    for span in spans
        .iter()
        .filter(|span| span.identifier.starts_with("vm."))
    {
        let location = format!("{VM_CHAPTER}:{}: `{}`", span.line, span.identifier);
        let Some(protected) = span.protected().filter(|protected| !protected.is_empty()) else {
            refusals.push(format!("{location} says nothing it protects"));
            continue;
        };
        for identifier in &protected {
            if !rules.iter().any(|rule| &rule.identifier == identifier) {
                refusals.push(format!(
                    "{location} protects `{identifier}`, which is no rule"
                ));
            }
        }
        // A walk through the rules of the chapter, each once, to a rule of
        // another chapter: a ring of `vm.` rules protecting each other
        // protects nothing.
        let mut visited = vec![span.identifier.clone()];
        let mut pending = protected;
        let mut reaches = false;
        while let Some(identifier) = pending.pop() {
            if !identifier.starts_with("vm.") {
                reaches |= rules.iter().any(|rule| rule.identifier == identifier);
            } else if !visited.contains(&identifier) {
                pending.extend(protected_by(&identifier));
                visited.push(identifier);
            }
        }
        if !reaches {
            refusals.push(format!(
                "{location} protects no rule outside the VM invariants"
            ));
        }
        let tested = references
            .iter()
            .any(|reference| reference.identifier == span.identifier);
        let detected = detectors
            .iter()
            .any(|detector| detector.rules.contains(&span.identifier));
        match (span.is_undetected(), tested || detected) {
            (false, false) => refusals.push(format!(
                "{location} has no test, no detector and no `{UNDETECTED}` line"
            )),
            (true, true) => refusals.push(format!(
                "{location} is marked undetected but has a detector or a test"
            )),
            _ => {}
        }
    }
    if refusals.is_empty() {
        Ok(())
    } else {
        Err(refusals.join("\n"))
    }
}

/// The detectors that name `identifier`.
fn detectors_of<'a>(identifier: &str, detectors: &'a [Detector]) -> Vec<&'a Detector> {
    detectors
        .iter()
        .filter(|detector| detector.rules.iter().any(|rule| rule == identifier))
        .collect()
}

/// A rule's tests and detectors as its anchor shows them: a list folded
/// shut by default, since a rule may name dozens of tests and the prose
/// around it is what a reader reads first, or `none yet`, or, for a rule
/// marked undetected, `none, undetected`. A Rust test shows its name and,
/// beneath it, its file; a golden its file alone; a detector its name and
/// its kind. The HTML stays on one line, so that Markdown takes it as one
/// HTML block.
fn listed_checks(tests: &[(String, String)], detectors: &[&Detector], undetected: bool) -> String {
    if tests.is_empty() && detectors.is_empty() {
        let text = if undetected {
            "none, undetected"
        } else {
            "none yet"
        };
        return format!("<span class=\"rule-tests\">tests: {text}</span>");
    }
    let count = |number: usize, noun: &str| {
        let plural = if number == 1 { "" } else { "s" };
        format!("{number} {noun}{plural}")
    };
    let mut counts = Vec::new();
    if !tests.is_empty() {
        counts.push(count(tests.len(), "test"));
    }
    if !detectors.is_empty() {
        counts.push(count(detectors.len(), "detector"));
    }
    let mut items: String = tests
        .iter()
        .map(|(file, name)| {
            if name.is_empty() {
                format!("<li><code>{}</code></li>", escape(file))
            } else {
                format!(
                    "<li><code>{}</code><span class=\"test-file\">{}</span></li>",
                    escape(name),
                    escape(file)
                )
            }
        })
        .collect();
    for detector in detectors {
        items.push_str(&format!(
            "<li><code>{}</code><span class=\"test-file\">{}</span></li>",
            escape(&detector.name),
            detector.kind.caption()
        ));
    }
    format!(
        "<details class=\"rule-tests\"><summary>{}</summary><ul>{items}</ul></details>",
        counts.join(", ")
    )
}

/// A chapter with each rule line made an anchor that lists the rule's checks.
fn processed_chapter(markdown: &str, references: &[Reference], detectors: &[Detector]) -> String {
    let undetected = undetected_in(markdown);
    let mut processed = String::new();
    let _ = walk_markdown(markdown, |_, line, outside| {
        match rule_line(line).filter(|_| outside) {
            Some(identifier) => {
                processed.push_str(&format!(
                    "<div class=\"rule\" id=\"r-{identifier}\"><a href=\"#r-{identifier}\">[{identifier}]</a> \
                     {}</div>\n\n",
                    listed_checks(
                        &tests_of(identifier, references),
                        &detectors_of(identifier, detectors),
                        undetected.iter().any(|known| known == identifier)
                    )
                ));
            }
            None => {
                processed.push_str(line);
                processed.push('\n');
            }
        }
    });
    processed
}

/// The appendix of the rules nothing checks, chapter by chapter: no test and
/// no detector names them. A rule marked `Undetected:` stays, and says so.
fn untested_rules(
    rules: &[Rule],
    references: &[Reference],
    detectors: &[Detector],
    undetected: &[String],
    chapters: &[PathBuf],
) -> String {
    let mut text = String::from(
        "# Untested rules\n\nThe rules no golden, no Rust test and no detector names yet, chapter by chapter.\n",
    );
    let mut any = false;
    for chapter in chapters {
        let untested: Vec<&Rule> = rules
            .iter()
            .filter(|rule| {
                &rule.chapter == chapter
                    && tests_of(&rule.identifier, references).is_empty()
                    && detectors_of(&rule.identifier, detectors).is_empty()
            })
            .collect();
        if untested.is_empty() {
            continue;
        }
        any = true;
        text.push_str(&format!("\n## {}\n\n", display(chapter)));
        for rule in untested {
            let page = display(chapter).replace(".md", ".html");
            let note = if undetected.contains(&rule.identifier) {
                " (undetected)"
            } else {
                ""
            };
            text.push_str(&format!(
                "- [`{}`]({page}#r-{}){note}\n",
                rule.identifier, rule.identifier
            ));
        }
    }
    if !any {
        text.push_str("\nEvery rule has a test.\n");
    }
    text
}

/// Fails unless `mdbook --version` names the release the book is rendered with.
fn check_mdbook() -> Result<(), String> {
    let wanted = format!(
        "the specification needs mdBook {MDBOOK_VERSION}: cargo install mdbook --locked --version {MDBOOK_VERSION}"
    );
    let output = std::process::Command::new("mdbook")
        .arg("--version")
        .output()
        .map_err(|_| wanted.clone())?;
    let version = String::from_utf8_lossy(&output.stdout);
    if version.trim() == format!("mdbook v{MDBOOK_VERSION}") {
        Ok(())
    } else {
        Err(format!("{wanted} (found {})", version.trim()))
    }
}

/// The book's `book.toml` pointed at the processed copy: its sources in
/// `src`, its HTML in `html`; refused when the text is not the form those
/// two edits expect, rather than left unpointed.
fn output_configuration(configuration: &str) -> Result<String, String> {
    let configuration = configuration.replace("\r\n", "\n");
    for expected in ["src = \".\"", "[build]\n"] {
        if !configuration.contains(expected) {
            return Err(format!(
                "docs/spec/book.toml: `{}` is where the copy is pointed, and it is missing",
                expected.trim_end()
            ));
        }
    }
    Ok(configuration
        .replace("src = \".\"", "src = \"src\"")
        .replace("[build]\n", "[build]\nbuild-dir = \"html\"\n"))
}

/// Checks the book and renders it into `output`: the processed chapters in
/// `output/src`, the HTML in `output/html`.
pub fn render(root: &Path, output: &Path) -> Result<(), String> {
    let book = root.join("docs/spec");
    let rules = book_rules(&book)?;
    let references = references_in(root)?;
    check_references(&rules, &references)?;
    check_pointers(&rules, &pointers_in(root)?)?;
    let detectors = detectors_in(&read(&book.join(VM_CHAPTER))?, Path::new(VM_CHAPTER))?;
    check_detectors(&rules, &detectors, &crate::STRESS_MODES)?;
    check_invariants(&book, &rules, &references, &detectors)?;
    check_links(&book)?;
    check_mdbook()?;
    let chapters = book_chapters(&book)?;
    let source = output.join("src");
    let mut undetected = Vec::new();
    for chapter in &chapters {
        undetected.extend(undetected_in(&read(&book.join(chapter))?));
    }
    let _ = std::fs::remove_dir_all(output);
    let write = |path: &Path, contents: &str| -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{}: cannot be created: {error}", display(parent)))?;
        }
        std::fs::write(path, contents)
            .map_err(|error| format!("{}: cannot be written: {error}", display(path)))
    };
    for chapter in &chapters {
        let markdown = read(&book.join(chapter))?;
        write(
            &source.join(chapter),
            &processed_chapter(&markdown, &references, &detectors),
        )?;
    }
    let mut summary = read(&book.join("SUMMARY.md"))?.replace("\r\n", "\n");
    if !summary.ends_with('\n') {
        summary.push('\n');
    }
    summary.push_str("- [Untested rules](untested-rules.md)\n");
    write(&source.join("SUMMARY.md"), &summary)?;
    write(
        &source.join("untested-rules.md"),
        &untested_rules(&rules, &references, &detectors, &undetected, &chapters),
    )?;
    let configuration = output_configuration(&read(&book.join("book.toml"))?)?;
    write(&output.join("book.toml"), &configuration)?;
    // `additional-css` is relative to the book's root, which the copy moves.
    write(&output.join("book.css"), &read(&book.join("book.css"))?)?;
    let status = std::process::Command::new("mdbook")
        .arg("build")
        .arg(output)
        .status()
        .map_err(|error| format!("mdbook build: could not start: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("mdbook build ({status})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory of the test's own below the system's temporary
    /// directory, removed when it is dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let path = std::env::temp_dir()
                .join(format!("ns-next-xtask-spec-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create the scratch directory");
            Scratch(path)
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().expect("a parent"))
                .expect("create the directory");
            std::fs::write(path, contents).expect("write the file");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn repository_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask lies inside the repository")
            .to_path_buf()
    }

    fn identifiers(rules: &[Rule]) -> Vec<&str> {
        rules.iter().map(|rule| rule.identifier.as_str()).collect()
    }

    #[test]
    fn a_rule_line_outside_a_fenced_block_is_a_rule() {
        let rules = rules_in(
            "intro\r\n\r\nr[lex.comment.line]\r\nA comment ...\r\n",
            Path::new("lexical.md"),
        )
        .expect("well formed");
        assert_eq!(
            rules,
            [Rule {
                identifier: "lex.comment.line".to_string(),
                chapter: PathBuf::from("lexical.md"),
                line: 3,
            }]
        );
    }

    #[test]
    fn a_rule_line_inside_a_fenced_block_is_text() {
        let rules = rules_in(
            "```\nr[lex.inside]\n```\n\n````text\n```\nr[lex.deeper]\n```\n````\nr[lex.after]\n",
            Path::new("lexical.md"),
        )
        .expect("well formed");
        assert_eq!(identifiers(&rules), ["lex.after"]);
    }

    /// CommonMark: a backtick fence's info string holds no backtick, so a
    /// line that opens with a code span of three backticks opens no fence.
    #[test]
    fn a_line_opening_with_a_code_span_is_no_fence() {
        let rules = rules_in(
            "r[syntax.first]\nrefused,\n``` `super` is followed (§5.10) ```.\n\nr[syntax.second]\n",
            Path::new("grammar.md"),
        )
        .expect("well formed");
        assert_eq!(identifiers(&rules), ["syntax.first", "syntax.second"]);
    }

    #[test]
    fn a_fence_inside_a_list_item_hides_its_rule_lines() {
        let rules = rules_in(
            "- an item:\n\n    ```text\n    r[lex.sample]\n    ```\n\nr[lex.after]\n",
            Path::new("lexical.md"),
        )
        .expect("well formed");
        assert_eq!(identifiers(&rules), ["lex.after"]);
    }

    /// A rule line stands at the start of its line, so an indented code
    /// block's `r[...]` is a sample.
    #[test]
    fn an_indented_rule_line_is_text() {
        let rules = rules_in("text\n\n    r[lex.indented]\n", Path::new("lexical.md"))
            .expect("well formed");
        assert!(rules.is_empty(), "{rules:?}");
    }

    #[test]
    fn a_rule_line_with_words_after_it_is_refused() {
        let refusal = rules_in("intro\nr[lex.a] and more\n", Path::new("lexical.md"))
            .expect_err("words after the rule");
        assert!(refusal.contains("lexical.md:2"), "{refusal}");
    }

    #[test]
    fn an_unclosed_fence_is_refused() {
        let refusal = rules_in("intro\n```text\nr[lex.a]\n", Path::new("lexical.md"))
            .expect_err("an unclosed fence");
        assert!(refusal.contains("lexical.md:2"), "{refusal}");
    }

    #[test]
    fn a_malformed_identifier_is_refused() {
        for (identifier, line) in [
            ("Lex.comment", 2),
            ("lex..comment", 2),
            ("lex.", 2),
            ("9lex", 2),
            ("lex.comment_line", 2),
            ("", 2),
        ] {
            let markdown = format!("intro\nr[{identifier}]\n");
            let refusal =
                rules_in(&markdown, Path::new("lexical.md")).expect_err("a malformed identifier");
            assert!(
                refusal.contains("lexical.md") && refusal.contains(&format!(":{line}")),
                "`{identifier}`: {refusal}"
            );
        }
    }

    #[test]
    fn a_duplicate_identifier_is_refused() {
        let scratch = Scratch::new("duplicate");
        scratch.write("SUMMARY.md", "# Summary\n\n- [A](a.md)\n- [B](b.md)\n");
        scratch.write("a.md", "# A\n\nr[syntax.outer.alone]\nText.\n");
        scratch.write("b.md", "# B\n\nr[syntax.outer.alone]\nText.\n");
        let refusal = book_rules(&scratch.0).expect_err("a duplicate");
        assert!(
            refusal.contains("syntax.outer.alone")
                && refusal.contains("a.md")
                && refusal.contains("b.md"),
            "{refusal}"
        );
    }

    #[test]
    fn a_chapter_missing_from_the_summary_is_refused() {
        let scratch = Scratch::new("missing");
        scratch.write("SUMMARY.md", "# Summary\n\n[Intro](intro.md)\n");
        scratch.write("intro.md", "# Intro\n");
        scratch.write("part/orphan.md", "# Orphan\n");
        let refusal = book_rules(&scratch.0).expect_err("an orphan chapter");
        assert!(refusal.contains("orphan.md"), "{refusal}");
    }

    #[test]
    fn a_summary_entry_without_a_file_is_refused() {
        let scratch = Scratch::new("absent");
        scratch.write("SUMMARY.md", "# Summary\n\n- [Gone](gone.md)\n");
        let refusal = book_rules(&scratch.0).expect_err("an absent chapter");
        assert!(refusal.contains("gone.md"), "{refusal}");
    }

    #[test]
    fn a_golden_and_a_rust_test_refer_to_rules() {
        let scratch = Scratch::new("references");
        scratch.write(
            "goldens/language/outer.toml",
            "stdout = '''\nrules = not this one\n'''\nrules = [\"syntax.outer.alone\", \"lex.identifier\"]\n",
        );
        scratch.write(
            "crates/ns-syntax/src/parser.rs",
            "fn code() {}\n\n    // spec: syntax.outer.send\n    #[test]\n    fn outer() {}\n",
        );
        let mut references = references_in(&scratch.0).expect("readable");
        references.sort_by(|left, right| left.identifier.cmp(&right.identifier));
        let found: Vec<(&str, usize)> = references
            .iter()
            .map(|reference| (reference.identifier.as_str(), reference.line))
            .collect();
        assert_eq!(
            found,
            [
                ("lex.identifier", 4),
                ("syntax.outer.alone", 4),
                ("syntax.outer.send", 3)
            ]
        );
    }

    /// The identifiers a golden's `.toml` names, read through `references_in`.
    fn golden_identifiers(name: &str, toml: &str) -> Result<Vec<String>, String> {
        let scratch = Scratch::new(name);
        scratch.write("goldens/a.toml", toml);
        Ok(references_in(&scratch.0)?
            .into_iter()
            .map(|reference| reference.identifier)
            .collect())
    }

    /// Every TOML spelling of the list counts, so that no reference a
    /// golden makes can escape the check.
    #[test]
    fn every_toml_spelling_of_a_goldens_rules_is_read() {
        for (name, toml) in [
            ("multi-line", "rules = [\n  \"lex.a\",\n  \"lex.b\",\n]\n"),
            ("commented", "rules = [\"lex.a\", \"lex.b\"] # the two\n"),
            (
                "after-a-quote",
                "stdout = \"\\\"\\\"\\\"\"\nrules = [\"lex.a\", \"lex.b\"]\n",
            ),
            (
                "literal",
                "stdout = '''\nrules = [\"lex.no\"]\n'''\nrules = ['lex.a', 'lex.b']\n",
            ),
        ] {
            assert_eq!(
                golden_identifiers(name, toml).expect(name),
                ["lex.a", "lex.b"],
                "{name}"
            );
        }
    }

    #[test]
    fn a_goldens_rules_that_are_no_list_of_identifiers_are_refused() {
        for (name, toml) in [
            ("string", "rules = \"lex.a\"\n"),
            ("number", "rules = [1]\n"),
            ("invalid", "rules = [\n"),
        ] {
            let refusal = golden_identifiers(name, toml).expect_err(name);
            assert!(refusal.contains("a.toml"), "{name}: {refusal}");
        }
    }

    #[test]
    fn a_reference_to_no_rule_is_reported() {
        let rules = [Rule {
            identifier: "syntax.outer.alone".to_string(),
            chapter: PathBuf::from("grammar.md"),
            line: 3,
        }];
        let references = [Reference {
            identifier: "syntax.nothing".to_string(),
            test: PathBuf::from("goldens/language/outer.toml"),
            name: String::new(),
            line: 4,
        }];
        let refusal = check_references(&rules, &references).expect_err("no such rule");
        assert!(
            refusal.contains("syntax.nothing") && refusal.contains("outer.toml"),
            "{refusal}"
        );
    }

    #[test]
    fn a_link_to_no_rule_is_refused() {
        let scratch = Scratch::new("links");
        scratch.write("SUMMARY.md", "# Summary\n\n- [A](a.md)\n- [B](part/b.md)\n");
        scratch.write(
            "a.md",
            "# A\n\nr[lex.here]\nSee [here](#r-lex.here) and [there](part/b.md#r-syntax.there).\n",
        );
        scratch.write(
            "part/b.md",
            "# B\n\nr[syntax.there]\nSee [a](../a.md#r-lex.here), [gone](../a.md#r-lex.gone) and [wrong](#r-lex.here).\n",
        );
        let refusal = check_links(&scratch.0).expect_err("two dangling links");
        assert!(refusal.contains("lex.gone"), "{refusal}");
        assert!(refusal.contains("part/b.md:4"), "{refusal}");
        assert_eq!(refusal.lines().count(), 2, "{refusal}");
    }

    #[test]
    fn a_link_to_no_chapter_is_refused() {
        let scratch = Scratch::new("chapter-links");
        scratch.write("SUMMARY.md", "# Summary\n\n- [A](a.md)\n- [B](part/b.md)\n");
        scratch.write(
            "a.md",
            "# A\n\nSee [B](part/b.md), [gone](part/gone.md) and [web](https://example.org/x.md).\n",
        );
        scratch.write("part/b.md", "# B\n\nSee [A](../a.md#a).\n");
        let refusal = check_links(&scratch.0).expect_err("one dangling link");
        assert!(
            refusal.contains("part/gone.md") && refusal.contains("a.md:3"),
            "{refusal}"
        );
        assert_eq!(refusal.lines().count(), 1, "{refusal}");
    }

    /// A `// spec:` line belongs to the test right below it, which the book
    /// names beside its file.
    #[test]
    fn a_rust_reference_stands_directly_above_a_test() {
        let scratch = Scratch::new("rust-references");
        scratch.write(
            "crates/a/src/lib.rs",
            "    // spec: lex.a\n    // spec: lex.b\n    #[test]\n    fn named() {}\n",
        );
        let references = references_in(&scratch.0).expect("readable");
        let found: Vec<(&str, &str)> = references
            .iter()
            .map(|reference| (reference.identifier.as_str(), reference.name.as_str()))
            .collect();
        assert_eq!(found, [("lex.a", "named"), ("lex.b", "named")]);
        scratch.write("crates/a/src/lib.rs", "// spec: lex.a\nfn helper() {}\n");
        let refusal = references_in(&scratch.0).expect_err("no test below");
        assert!(refusal.contains("lib.rs:1"), "{refusal}");
    }

    #[test]
    fn a_rules_tests_are_a_list_folded_shut() {
        let reference = |identifier: &str, name: &str| Reference {
            identifier: identifier.to_string(),
            test: PathBuf::from("crates/a/tests/b.rs"),
            name: name.to_string(),
            line: 1,
        };
        let references = [
            reference("lex.a", "first"),
            reference("lex.a", "second<T>"),
            reference("lex.b", ""),
        ];
        let processed = processed_chapter("r[lex.a]\nr[lex.b]\nr[lex.c]\n", &references, &[]);
        let lines: Vec<&str> = processed.lines().filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            [
                "<div class=\"rule\" id=\"r-lex.a\"><a href=\"#r-lex.a\">[lex.a]</a> \
                 <details class=\"rule-tests\"><summary>2 tests</summary><ul>\
                 <li><code>first</code><span class=\"test-file\">crates/a/tests/b.rs</span></li>\
                 <li><code>second&lt;T&gt;</code><span class=\"test-file\">crates/a/tests/b.rs</span></li>\
                 </ul></details></div>",
                "<div class=\"rule\" id=\"r-lex.b\"><a href=\"#r-lex.b\">[lex.b]</a> \
                 <details class=\"rule-tests\"><summary>1 test</summary><ul>\
                 <li><code>crates/a/tests/b.rs</code></li></ul></details></div>",
                "<div class=\"rule\" id=\"r-lex.c\"><a href=\"#r-lex.c\">[lex.c]</a> \
                 <span class=\"rule-tests\">tests: none yet</span></div>",
            ]
        );
    }

    #[test]
    fn a_book_toml_that_cannot_be_pointed_at_the_copy_is_refused() {
        let pointed =
            output_configuration("[book]\nsrc = \".\"\n\n[build]\ncreate-missing = false\n")
                .expect("the book's own form");
        assert!(pointed.contains("src = \"src\"") && pointed.contains("build-dir = \"html\""));
        output_configuration("[book]\nsrc=\".\"\n").expect_err("another spelling");
    }

    #[test]
    fn every_link_of_the_book_names_a_rule_of_its_chapter() {
        check_links(&repository_root().join("docs/spec")).expect("every rule link resolves");
    }

    #[test]
    fn every_rule_of_the_book_is_well_formed_and_unique() {
        book_rules(&repository_root().join("docs/spec")).expect("the book's rules");
    }

    #[test]
    fn every_reference_names_a_rule_of_the_book() {
        let root = repository_root();
        let rules = book_rules(&root.join("docs/spec")).expect("the book's rules");
        let references = references_in(&root).expect("the tests' references");
        check_references(&rules, &references).expect("every reference names a rule");
    }

    /// A scratch repository with a one-rule book and `contents` as the
    /// design document `docs/design/a.md`.
    fn pointer_scratch(name: &str, document: &str, contents: &str) -> Scratch {
        let scratch = Scratch::new(name);
        scratch.write(
            "docs/spec/SUMMARY.md",
            "# Summary

- [A](a.md)
",
        );
        scratch.write(
            "docs/spec/a.md",
            "r[vm.sample]
A rule.
",
        );
        scratch.write(document, contents);
        scratch
    }

    #[test]
    fn a_pointer_to_a_rule_of_the_book_is_accepted() {
        let scratch = pointer_scratch(
            "pointer-ok",
            "docs/design/a.md",
            "see `r[vm.sample]`
",
        );
        let rules = book_rules(&scratch.0.join("docs/spec")).expect("the book");
        let pointers = pointers_in(&scratch.0).expect("the pointers");
        assert_eq!(pointers.len(), 1);
        assert_eq!(pointers[0].identifier, "vm.sample");
        assert_eq!(pointers[0].line, 1);
        check_pointers(&rules, &pointers).expect("a rule of the book");
    }

    #[test]
    fn a_pointer_to_no_rule_is_refused() {
        let scratch = pointer_scratch(
            "pointer-none",
            "docs/design/a.md",
            "one
two
`r[vm.none]`
",
        );
        let rules = book_rules(&scratch.0.join("docs/spec")).expect("the book");
        let pointers = pointers_in(&scratch.0).expect("the pointers");
        let refusal = check_pointers(&rules, &pointers).expect_err("no such rule");
        assert_eq!(
            refusal,
            "docs/design/a.md:3: `r[vm.none]` names no rule of the specification"
        );
    }

    #[test]
    fn a_pointer_in_a_fenced_block_is_a_sample() {
        let scratch = pointer_scratch(
            "pointer-fence",
            "docs/design/a.md",
            "```
`r[vm.none]`
```
",
        );
        assert!(pointers_in(&scratch.0).expect("the pointers").is_empty());
    }

    #[test]
    fn a_bare_identifier_is_no_pointer() {
        let scratch = pointer_scratch(
            "pointer-bare",
            "docs/design/a.md",
            "`vm.rs` and `image.bound.sizing` and `ns-next-analyzer[.exe]`
",
        );
        assert!(pointers_in(&scratch.0).expect("the pointers").is_empty());
    }

    #[test]
    fn a_malformed_pointer_is_refused() {
        let scratch = pointer_scratch(
            "pointer-bad",
            "CLAUDE.md",
            "`r[Vm.X]`
",
        );
        let refusal = pointers_in(&scratch.0).expect_err("malformed");
        assert_eq!(refusal, "CLAUDE.md:1: `r[Vm.X]` is no rule identifier");
    }

    #[test]
    fn every_pointer_names_a_rule_of_the_book() {
        let root = repository_root();
        let rules = book_rules(&root.join("docs/spec")).expect("the book's rules");
        let pointers = pointers_in(&root).expect("the documents' pointers");
        check_pointers(&rules, &pointers).expect("every pointer names a rule");
    }

    /// A scratch book of a `vm-invariants.md` made of `vm_chapter`, and a
    /// `semantics.md` declaring `semantics.sample` and `extra`.
    fn vm_scratch(name: &str, vm_chapter: &str, extra: &str) -> Scratch {
        let scratch = Scratch::new(name);
        scratch.write(
            "SUMMARY.md",
            "# Summary\n\n- [Semantics](semantics.md)\n- [VM](vm-invariants.md)\n",
        );
        scratch.write(
            "semantics.md",
            &format!("# Semantics\n\nr[semantics.sample]\nText.\n\n{extra}"),
        );
        scratch.write("vm-invariants.md", vm_chapter);
        scratch
    }

    /// The table of detectors with `rows`, each a line of the table.
    fn detector_table(rows: &[&str]) -> String {
        format!(
            "```spec-table vm-detectors\n| Detector | Kind | What it checks | Rules |\n|---|---|---|---|\n{}\n```\n",
            rows.join("\n")
        )
    }

    fn vm_check(scratch: &Scratch, references: &[Reference]) -> Result<(), String> {
        let rules = book_rules(&scratch.0).expect("the book's rules");
        let markdown = std::fs::read_to_string(scratch.0.join("vm-invariants.md")).expect("read");
        let detectors =
            detectors_in(&markdown, Path::new("vm-invariants.md")).expect("the detectors");
        check_invariants(&scratch.0, &rules, references, &detectors)
    }

    const PROTECTS_SAMPLE: &str =
        "Protects: [`semantics.sample`](semantics.md#r-semantics.sample)\n";

    #[test]
    fn a_vm_rule_without_protects_is_refused() {
        let chapter = format!(
            "# VM\n\nr[vm.a]\nUndetected: no way.\n\n{}",
            detector_table(&[])
        );
        let refusal = vm_check(&vm_scratch("vm-bare", &chapter, ""), &[]).expect_err("no Protects");
        assert!(
            refusal.contains("vm-invariants.md:3: `vm.a` says nothing it protects"),
            "{refusal}"
        );
    }

    #[test]
    fn a_protection_cycle_among_vm_rules_is_refused() {
        let chapter = "# VM\n\nr[vm.a]\nProtects: [`vm.b`](#r-vm.b)\nUndetected: x.\n\nr[vm.b]\nProtects: [`vm.a`](#r-vm.a)\nUndetected: x.\n";
        let refusal = vm_check(&vm_scratch("vm-cycle", chapter, ""), &[]).expect_err("a cycle");
        assert!(
            refusal.contains("`vm.a` protects no rule outside the VM invariants")
                && refusal.contains("`vm.b` protects no rule outside the VM invariants"),
            "{refusal}"
        );
    }

    #[test]
    fn a_vm_rule_protecting_through_another_is_accepted() {
        let chapter = format!(
            "# VM\n\nr[vm.a]\nProtects: [`vm.b`](#r-vm.b)\n\nr[vm.b]\n{PROTECTS_SAMPLE}\n{}",
            detector_table(&["| MOVES | stress | moves | `vm.a`, `vm.b` |"])
        );
        vm_check(&vm_scratch("vm-through", &chapter, ""), &[]).expect("accepted");
    }

    #[test]
    fn a_vm_rule_with_nothing_that_checks_it_is_refused() {
        let chapter = format!("# VM\n\nr[vm.a]\n{PROTECTS_SAMPLE}");
        let refusal =
            vm_check(&vm_scratch("vm-nothing", &chapter, ""), &[]).expect_err("unchecked");
        assert!(
            refusal.contains("`vm.a` has no test, no detector and no `Undetected:` line"),
            "{refusal}"
        );
    }

    #[test]
    fn an_undetected_rule_with_a_detector_is_refused() {
        let chapter = format!(
            "# VM\n\nr[vm.a]\n{PROTECTS_SAMPLE}Undetected: none.\n\n{}",
            detector_table(&["| MOVES | stress | moves | `vm.a` |"])
        );
        let refusal = vm_check(&vm_scratch("vm-both", &chapter, ""), &[]).expect_err("both");
        assert!(
            refusal.contains("`vm.a` is marked undetected but has a detector or a test"),
            "{refusal}"
        );
    }

    #[test]
    fn an_undetected_rule_alone_is_accepted() {
        let chapter = format!("# VM\n\nr[vm.a]\n{PROTECTS_SAMPLE}Undetected: none.\n");
        vm_check(&vm_scratch("vm-alone", &chapter, ""), &[]).expect("accepted");
    }

    #[test]
    fn a_stress_mode_without_a_row_is_refused() {
        let detectors = [Detector {
            name: "MOVES".to_string(),
            kind: DetectorKind::Stress,
            rules: Vec::new(),
            line: 5,
        }];
        let refusal =
            check_detectors(&[], &detectors, &["MOVES", "JIT"]).expect_err("a missing row");
        assert!(
            refusal.contains("the stress mode JIT has no row in the table of detectors"),
            "{refusal}"
        );
    }

    #[test]
    fn a_row_naming_no_stress_mode_is_refused() {
        let markdown = detector_table(&["| ZONES | stress | zones | |"]);
        let detectors = detectors_in(&markdown, Path::new("vm-invariants.md")).expect("rows");
        let refusal = check_detectors(&[], &detectors, &[]).expect_err("no such mode");
        assert!(
            refusal.contains("vm-invariants.md:4: ZONES is no stress mode the stress gate runs"),
            "{refusal}"
        );
    }

    #[test]
    fn a_detector_naming_no_vm_rule_is_refused() {
        let scratch = vm_scratch("vm-nonvm", "# VM\n", "");
        let rules = book_rules(&scratch.0).expect("rules");
        let markdown = detector_table(&["| Compiler | compiler | rustc | `semantics.sample` |"]);
        let detectors = detectors_in(&markdown, Path::new("vm-invariants.md")).expect("rows");
        let refusal = check_detectors(&rules, &detectors, &[]).expect_err("not a vm rule");
        assert!(
            refusal.contains("`semantics.sample` is no rule of the VM invariants"),
            "{refusal}"
        );
    }

    #[test]
    fn a_vm_rule_outside_its_chapter_is_refused() {
        let scratch = vm_scratch("vm-stray", "# VM\n", "r[vm.stray]\nText.\n");
        let refusal = vm_check(&scratch, &[]).expect_err("a stray rule");
        assert!(
            refusal.contains("`vm.stray` is declared outside the VM invariants"),
            "{refusal}"
        );
    }

    fn detector(name: &str, kind: DetectorKind) -> Detector {
        Detector {
            name: name.to_string(),
            kind,
            rules: vec!["vm.a".to_string()],
            line: 1,
        }
    }

    #[test]
    fn a_rules_detectors_are_listed_beside_its_tests() {
        let references = [Reference {
            identifier: "vm.a".to_string(),
            test: PathBuf::from("crates/a/tests/b.rs"),
            name: "first".to_string(),
            line: 1,
        }];
        let one = processed_chapter(
            "r[vm.a]\n",
            &references,
            &[detector("SCAVENGE", DetectorKind::Stress)],
        );
        assert!(
            one.contains("<summary>1 test, 1 detector</summary>"),
            "{one}"
        );
        assert!(
            one.contains(
                "<li><code>SCAVENGE</code><span class=\"test-file\">stress mode</span></li>"
            ),
            "{one}"
        );
        let several = processed_chapter(
            "r[vm.a]\n",
            &references,
            &[
                detector("SCAVENGE", DetectorKind::Stress),
                detector("poison", DetectorKind::Debug),
                detector("borrow", DetectorKind::Compiler),
            ],
        );
        assert!(
            several.contains("<summary>1 test, 3 detectors</summary>"),
            "{several}"
        );
        assert!(
            several.contains("<span class=\"test-file\">debug build</span>"),
            "{several}"
        );
        assert!(
            several.contains("<span class=\"test-file\">Rust compiler</span>"),
            "{several}"
        );
        let alone = processed_chapter(
            "r[vm.a]\n",
            &[],
            &[detector("SCAVENGE", DetectorKind::Stress)],
        );
        assert!(alone.contains("<summary>1 detector</summary>"), "{alone}");
    }

    #[test]
    fn an_undetected_rule_shows_so() {
        let processed = processed_chapter("r[vm.a]\nProtects: x.\nUndetected: none.\n", &[], &[]);
        assert!(
            processed.contains("<span class=\"rule-tests\">tests: none, undetected</span>"),
            "{processed}"
        );
    }

    #[test]
    fn a_rule_with_a_detector_is_not_untested() {
        let rule = |identifier: &str| Rule {
            identifier: identifier.to_string(),
            chapter: PathBuf::from("vm-invariants.md"),
            line: 1,
        };
        let rules = [rule("vm.a"), rule("vm.b"), rule("vm.c")];
        let text = untested_rules(
            &rules,
            &[],
            &[detector("SCAVENGE", DetectorKind::Stress)],
            &["vm.b".to_string()],
            &[PathBuf::from("vm-invariants.md")],
        );
        assert!(!text.contains("[`vm.a`]"), "{text}");
        assert!(text.contains("#r-vm.b) (undetected)\n"), "{text}");
        assert!(text.contains("[`vm.c`]"), "{text}");
    }

    #[test]
    fn every_vm_rule_protects_and_is_checked() {
        let root = repository_root();
        let book = root.join("docs/spec");
        let rules = book_rules(&book).expect("the book's rules");
        let references = references_in(&root).expect("the tests' references");
        let markdown = std::fs::read_to_string(book.join("vm-invariants.md")).expect("the chapter");
        let detectors =
            detectors_in(&markdown, Path::new("vm-invariants.md")).expect("the detectors");
        check_detectors(&rules, &detectors, &crate::STRESS_MODES).expect("the detectors");
        check_invariants(&book, &rules, &references, &detectors).expect("the invariants");
    }
}
