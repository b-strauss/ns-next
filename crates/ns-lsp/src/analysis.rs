//! One document's analysis: the errors `ns-next` would report before running
//! it, and what the resolver knows about each name.

use std::panic::{catch_unwind, AssertUnwindSafe};

use ns_syntax::{Diagnostic, Severity, Span};

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
            let detail = payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "no message".to_string());
            Analysis {
                findings: vec![Finding {
                    span: Span::new(0, 0),
                    message: format!(
                        "internal error in ns-next-lsp: {detail}; this is an ns-next defect, please report it"
                    ),
                    severity: Severity::Error,
                }],
                classified: Vec::new(),
            }
        }
    }
}

fn analyze_unguarded(text: &str) -> Analysis {
    let parsed = ns_syntax::parse_for_editor(text);
    // The resolver runs on a recovered module too. None of its errors can
    // come from the recovery, which never invents a declaration
    // (`docs/design/frontend.md`, "Recovery").
    let resolution = ns_runtime::resolve_for_editor(parsed.module);
    let classified =
        semantic_tokens::classify(&resolution.module, &resolution.bindings, &parsed.tokens);
    let mut findings: Vec<Finding> = parsed.diagnostics.into_iter().map(Finding::from).collect();
    findings.extend(resolution.errors.into_iter().map(|error| Finding {
        span: error.span,
        message: error.message,
        severity: Severity::Error,
    }));
    Analysis {
        findings,
        classified,
    }
}

#[cfg(test)]
mod tests {
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
        assert!(analysis
            .classified
            .iter()
            .any(|classified| classified.span.start == parameter_use));
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
