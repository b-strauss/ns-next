//! One document's analysis: the errors `ns-next` would report before running
//! it, and what the resolver knows about each name.

use std::panic::{AssertUnwindSafe, catch_unwind};

use std::collections::HashMap;

use ns_syntax::{Diagnostic, Severity, Span};

use crate::document_symbols::{self, Symbol};
use crate::folding::{self, Fold};
use crate::hover::{Declared, Reference};
use crate::semantic_tokens::{self, Classified};

/// One error to show, with the message `ns-next` itself produces.
pub struct Finding {
    pub span: Span,
    pub message: String,
    pub severity: Severity,
}

impl From<Diagnostic> for Finding {
    fn from(diagnostic: Diagnostic) -> Finding {
        Finding {
            span: diagnostic.span,
            message: diagnostic.message,
            severity: diagnostic.severity,
        }
    }
}

pub struct Analysis {
    pub findings: Vec<Finding>,
    pub classified: Vec<Classified>,
    /// Each name placed at a declaration of the document, and each such
    /// declaration, for the hover (`hover.rs`).
    pub references: Vec<Reference>,
    pub declarations: HashMap<Span, Declared>,
    pub folds: Vec<Fold>,
    pub symbols: Vec<Symbol>,
    pub hints: Vec<crate::hints::Hint>,
}

/// Every error `ns-next run` would refuse `text` for before running it --
/// the lexer's, the parser's and the resolver's, together -- and every name
/// the resolver can place. A source with errors is analysed as recovered,
/// so the colours survive while a file is typed. Never panics: see
/// `guarded`.
pub fn analyze(text: &str) -> Analysis {
    guarded(text, analyze_unguarded)
}

/// Runs `analyzer`, turning a panic into one finding at the start of the
/// file. An editor sends invalid text all the time and `ns-syntax` promises
/// diagnostics rather than panics for it; a panic here is therefore an
/// ns-next defect, and it must be visible without taking the server, and the
/// editor's errors for every other file, down with it.
pub(crate) fn guarded(text: &str, analyzer: impl FnOnce(&str) -> Analysis) -> Analysis {
    match catch_unwind(AssertUnwindSafe(|| analyzer(text))) {
        Ok(analysis) => analysis,
        Err(payload) => {
            let detail = panic_detail(payload.as_ref());
            Analysis {
                findings: vec![Finding {
                    span: Span::new(0, 0),
                    message: format!(
                        "internal error in ns-next-analyzer: {detail}; this is an ns-next defect, please report it"
                    ),
                    severity: Severity::Error,
                }],
                classified: Vec::new(),
                references: Vec::new(),
                declarations: HashMap::new(),
                folds: Vec::new(),
                symbols: Vec::new(),
                hints: Vec::new(),
            }
        }
    }
}

/// The message a panic carried, or "no message".
pub(crate) fn panic_detail(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".to_string())
}

fn analyze_unguarded(text: &str) -> Analysis {
    let ns_syntax::EditorParse {
        tokens,
        module,
        comments,
        diagnostics,
    } = ns_syntax::parse_for_editor(text);
    // The resolver runs on a recovered module too. Recovery adds no
    // declaration the source does not contain, so it causes no duplicate or
    // clashing-member error; an unbound name is a send in Newspeak, not an
    // error (`docs/design/frontend.md`, "Recovery").
    let resolution = ns_runtime::resolve_for_editor(module);
    let semantic_tokens::Classification {
        classified,
        references,
        mut declarations,
    } = semantic_tokens::classify(&resolution.module, &resolution.bindings, &tokens, text);
    let module_metadata = crate::module_metadata::check(&resolution.module, text);
    for (declaration, metadata) in module_metadata.shown {
        if let Some(declared) = declarations.get_mut(&declaration) {
            declared.module_metadata = Some(metadata);
        }
    }
    let folds = folding::folds(&resolution.module, &comments, text);
    let symbols = document_symbols::symbols(&resolution.module, &resolution.bindings);
    // Only a source that parses without error gets hints: their fixes are
    // made against the tree `parse` answers.
    let hints = crate::hints::hints_of(text);
    let mut findings: Vec<Finding> = diagnostics.into_iter().map(Finding::from).collect();
    findings.extend(resolution.errors.into_iter().map(|error| Finding {
        span: error.span,
        message: error.message,
        severity: Severity::Error,
    }));
    findings.extend(crate::doc_tags::findings(
        &resolution.module,
        &comments,
        text,
    ));
    findings.extend(module_metadata.findings);
    Analysis {
        findings,
        classified,
        references,
        declarations,
        folds,
        symbols,
        hints,
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::cast_possible_truncation,
        reason = "a test's values are small literals and lengths it built"
    )]

    use super::*;

    fn messages(text: &str) -> Vec<String> {
        analyze(text)
            .findings
            .into_iter()
            .map(|finding| finding.message)
            .collect()
    }

    #[test]
    fn a_clean_module_has_no_findings() {
        let text = indoc::indoc! {"
            class Module = Object new () () : ()
        "};
        assert!(messages(text).is_empty(), "{:?}", messages(text));
    }

    #[test]
    fn a_lexer_error_is_reported_where_it_is() {
        let text = indoc::indoc! {"
            class Module = Object new () ( public broken = ( ^'unterminated ) ) : ()
        "};
        let analysis = analyze(text);
        // The lexer's findings come first; the parser's follow, since the
        // string runs to the end of the file.
        assert_eq!(analysis.findings[0].message, "unterminated string literal");
        assert_eq!(
            analysis.findings[0].span.start as usize,
            text.find("'unterminated").expect("the literal")
        );
    }

    /// A syntax error no longer takes the semantic colours with it.
    #[test]
    fn a_syntax_error_keeps_the_names_classified() {
        let text = indoc::indoc! {"
            class Module = () (
                broken = (
                    ^1
                echo: argument = ( ^argument )
            ) : ()
        "};
        let analysis = analyze(text);
        assert!(!analysis.findings.is_empty());
        let parameter_use = text.rfind("argument").expect("the use") as u32;
        assert!(
            analysis
                .classified
                .iter()
                .any(|classified| classified.span.start == parameter_use)
        );
    }

    #[test]
    fn a_parser_error_is_reported() {
        let text = indoc::indoc! {"
            class Module = Object new ()
        "};
        let found = messages(text);
        // Recovery may add further errors after the first; the first one is
        // the parser's own message for the missing instance side.
        assert_eq!(
            found.first().map(String::as_str),
            Some("expected the instance side's `( ... )` after the class header (§6.3)"),
            "{found:?}"
        );
    }

    #[test]
    fn a_resolver_error_is_reported() {
        let text = indoc::indoc! {"
            class Module = Object new () ( public broken = ( ^outer Missing foo ) ) : ()
        "};
        let found = messages(text);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("outer Missing"), "{}", found[0]);
    }

    /// The root rule is a resolve error, so the editor underlines a module
    /// root's offending clause before anything runs.
    #[test]
    fn a_root_rule_violation_is_reported_at_the_clause() {
        let text = indoc::indoc! {"
            class Module = Array new () () : ()
        "};
        let analysis = analyze(text);
        assert_eq!(analysis.findings.len(), 1);
        let finding = &analysis.findings[0];
        assert_eq!(
            finding.message,
            "a module's root class may inherit only Object or Value, or write Object <: Value, \
             and its superclass clause names Array"
        );
        let start = text.find("Array new").expect("the superclass clause");
        assert_eq!(
            (finding.span.start as usize, finding.span.end as usize),
            (start, start + "Array new".len())
        );
    }

    /// A chain of messages nests a send per message through its receivers,
    /// as deep as the source is long, valid or not: `1 + 1 abs + …`, and a
    /// run of unclosed methods past the parser's depth limit, which the
    /// first method's statement takes as one chain. Walked by recursion, a
    /// valid 40,000-message chain overflowed the server's 8 MiB.
    #[test]
    fn a_chain_as_long_as_the_source_is_analysed_on_a_small_stack() {
        const STACK_BYTES: usize = 1024 * 1024;
        let valid = indoc::formatdoc! {"
            class Module = () (
                sum = ( ^1{links} )
            ) : ()
            ",
            links = " + 1 abs".repeat(50_000),
        };
        let unclosed = indoc::formatdoc! {"
            class Module = () (
            {methods}",
            methods = "    m = ( ^1\n".repeat(10_000),
        };
        let (valid_findings, unclosed_findings) = std::thread::Builder::new()
            .stack_size(STACK_BYTES)
            .spawn(move || {
                let findings = |text: &str| -> Vec<String> {
                    analyze(text)
                        .findings
                        .into_iter()
                        .map(|finding| finding.message)
                        .collect()
                };
                (findings(&valid), findings(&unclosed))
            })
            .expect("spawn the analysing thread")
            .join()
            .expect("the analysis answers on a small stack");
        assert!(valid_findings.is_empty(), "{:?}", valid_findings.first());
        assert_eq!(unclosed_findings.len(), 10_001);
        assert!(
            unclosed_findings
                .iter()
                .all(|message| !message.contains("internal error"))
        );
    }

    #[test]
    fn a_panic_becomes_one_internal_error_finding() {
        let analysis = guarded("anything", |_| panic!("the analyzer broke"));
        assert_eq!(analysis.findings.len(), 1);
        assert!(
            analysis.findings[0].message.contains("internal error"),
            "{}",
            analysis.findings[0].message
        );
        assert!(
            analysis.findings[0].message.contains("the analyzer broke"),
            "{}",
            analysis.findings[0].message
        );
        assert_eq!(analysis.findings[0].span, ns_syntax::Span::new(0, 0));
    }
}
