//! The chapter of the VM invariants: its table of detectors and the form
//! each of its rules takes (`docs/design/specification.md`, "The fifth
//! batch's decisions").

use super::{Reference, Rule, display, fence_of, read, rule_line, walk_markdown};
use std::path::Path;

/// The chapter that holds the VM invariants, and the only one that may
/// declare a `vm.` rule.
pub(super) const VM_CHAPTER: &str = "vm-invariants.md";

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
    pub(super) fn caption(self) -> &'static str {
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
    let _ = walk_markdown(markdown, |line, text, outside| {
        if outside {
            return;
        }
        // A line of a fenced block while no fence is open is the one that
        // opens it.
        let Some((character, length)) = fence else {
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
        };
        if let Some((closing, closing_length)) = fence_of(text)
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
            let separator = cells.len() == DETECTOR_HEADER.len()
                && cells.iter().all(|cell| {
                    !cell.trim_matches(':').is_empty()
                        && cell.trim_matches(':').chars().all(|next| next == '-')
                });
            if !separator {
                refusals.push(format!(
                    "{}: the line under the header of the table of detectors is its separator row, `|---|---|---|---|`",
                    location(line)
                ));
            }
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
        // A row that names no rule finds no breach the book can list, so it
        // says nothing about any rule's coverage.
        if detector.rules.is_empty() {
            refusals.push(format!(
                "{location}: the detector {} names no rule",
                detector.name
            ));
        }
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
pub(super) fn undetected_in(markdown: &str) -> Vec<String> {
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
pub(super) fn detectors_of<'a>(identifier: &str, detectors: &'a [Detector]) -> Vec<&'a Detector> {
    detectors
        .iter()
        .filter(|detector| detector.rules.iter().any(|rule| rule == identifier))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::tests::{Scratch, repository_root};
    use super::super::*;

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
    fn a_detector_naming_no_rule_is_refused() {
        let detectors = table_of(&["| MOVES | stress | moves | |"]).expect("rows");
        let refusal = check_detectors(&[], &detectors, &["MOVES"]).expect_err("an empty cell");
        assert!(
            refusal.contains("vm-invariants.md:4: the detector MOVES names no rule"),
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

    fn table_of(rows: &[&str]) -> Result<Vec<Detector>, String> {
        detectors_in(&detector_table(rows), Path::new("vm-invariants.md"))
    }

    #[test]
    fn adjacent_fenced_blocks_are_told_apart() {
        let after = "```text\nx\n```\n```spec-table vm-detectors\n| Detector | Kind | What it checks | Rules |\n|---|---|---|---|\n| MOVES | stress | m | |\n```\n";
        let detectors = detectors_in(after, Path::new("vm-invariants.md")).expect("the table");
        assert_eq!(detectors.len(), 1);
        assert_eq!(detectors[0].line, 7);
        let before = "```spec-table vm-detectors\n| Detector | Kind | What it checks | Rules |\n|---|---|---|---|\n| MOVES | stress | m | |\n```\n```text\n| JIT | stress | j | |\n```\n";
        let detectors = detectors_in(before, Path::new("vm-invariants.md")).expect("the table");
        assert_eq!(detectors.len(), 1);
    }

    #[test]
    fn a_second_table_is_refused() {
        let markdown = format!("{}\n{}", detector_table(&[]), detector_table(&[]));
        let refusal = detectors_in(&markdown, Path::new("vm-invariants.md")).expect_err("two");
        assert!(refusal.contains("a second table of detectors"), "{refusal}");
    }

    #[test]
    fn a_malformed_header_is_refused() {
        let markdown = "```spec-table vm-detectors\n| Name | Kind | What it checks | Rules |\n|---|---|---|---|\n```\n";
        let refusal = detectors_in(markdown, Path::new("vm-invariants.md")).expect_err("header");
        assert!(
            refusal.contains("vm-invariants.md:2: the table of detectors begins with the header"),
            "{refusal}"
        );
    }

    #[test]
    fn a_line_that_is_no_separator_row_is_refused() {
        let markdown = "```spec-table vm-detectors\n| Detector | Kind | What it checks | Rules |\n| MOVES | stress | m | |\n```\n";
        let refusal = detectors_in(markdown, Path::new("vm-invariants.md")).expect_err("separator");
        assert!(
            refusal.contains("vm-invariants.md:3: the line under the header"),
            "{refusal}"
        );
        let short = "```spec-table vm-detectors\n| Detector | Kind | What it checks | Rules |\n|---|---|\n```\n";
        detectors_in(short, Path::new("vm-invariants.md")).expect_err("a short separator");
        let aligned = "```spec-table vm-detectors\n| Detector | Kind | What it checks | Rules |\n|:--|--:|:-:|---|\n```\n";
        detectors_in(aligned, Path::new("vm-invariants.md")).expect("aligned separators");
    }

    #[test]
    fn a_kind_that_is_none_is_refused() {
        let refusal = table_of(&["| MOVES | fuzz | m | |"]).expect_err("kind");
        assert!(
            refusal.contains("vm-invariants.md:4: `fuzz` is no kind of detector"),
            "{refusal}"
        );
    }

    #[test]
    fn a_row_with_the_wrong_number_of_cells_is_refused() {
        let refusal = table_of(&["| MOVES | stress | m |"]).expect_err("cells");
        assert!(refusal.contains("has 3 cells, not 4"), "{refusal}");
    }

    #[test]
    fn a_stress_mode_with_two_rows_is_refused() {
        let detectors =
            table_of(&["| MOVES | stress | m | |", "| MOVES | stress | m | |"]).expect("rows");
        let refusal = check_detectors(&[], &detectors, &["MOVES"]).expect_err("two rows");
        assert!(
            refusal.contains("vm-invariants.md:5: the stress mode MOVES has more than one row"),
            "{refusal}"
        );
    }

    #[test]
    fn a_protects_link_naming_no_rule_is_refused() {
        let chapter =
            "# VM\n\nr[vm.a]\nProtects: [`x`](semantics.md#r-semantics.gone)\nUndetected: x.\n";
        let refusal = vm_check(&vm_scratch("vm-gone", chapter, ""), &[]).expect_err("no rule");
        assert!(
            refusal.contains("`vm.a` protects `semantics.gone`, which is no rule"),
            "{refusal}"
        );
    }

    #[test]
    fn a_rule_line_and_a_heading_in_a_fence_leave_a_span_alone() {
        let chapter = format!(
            "# VM\n\nr[vm.a]\n{PROTECTS_SAMPLE}\n```\n# not a heading\nr[vm.fake]\n```\n\nUndetected: yes.\n"
        );
        vm_check(&vm_scratch("vm-fenced", &chapter, ""), &[]).expect("one rule, undetected");
        let hidden = format!("# VM\n\nr[vm.a]\n{PROTECTS_SAMPLE}\n```\nUndetected: no.\n```\n");
        let refusal = vm_check(&vm_scratch("vm-hidden", &hidden, ""), &[]).expect_err("hidden");
        assert!(
            refusal.contains("`vm.a` has no test, no detector"),
            "{refusal}"
        );
    }
}
