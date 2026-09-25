//! The server's analysis over every Newspeak module the repository holds:
//! the resolver entry must agree with `resolve`, and nothing -- not even a
//! module cut off mid-way, which is what a file looks like while it is typed
//! -- may produce an internal error.

use std::path::{Path, PathBuf};

fn modules() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    collect(&root.join("corpus/modules"), &mut files);
    collect(&root.join("goldens"), &mut files);
    files.push(root.join("crates/ns-runtime/core-library/Kernel.ns"));
    files.sort();
    assert!(files.len() > 200, "found only {} modules", files.len());
    files
}

fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("reading {}: {error}", directory.display()));
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "ns") {
            files.push(path);
        }
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn the_editor_entry_agrees_with_resolve() {
    for path in modules() {
        let text = read(&path);
        let Ok(module) = ns_syntax::parse(&text) else {
            continue;
        };
        let resolution = ns_runtime::resolve_for_editor(module.clone());
        let origin = ns_runtime::SourceOrigin::File(path.clone());
        match ns_runtime::resolve(module, text.as_str().into(), origin) {
            Ok(resolved) => {
                assert!(
                    resolution.errors.is_empty(),
                    "{}: resolve found no error, the editor entry did",
                    path.display()
                );
                assert!(
                    *resolved.bindings == resolution.bindings,
                    "{}: the bindings differ",
                    path.display()
                );
            }
            Err(errors) => {
                let expected: Vec<_> = errors
                    .iter()
                    .map(|error| (error.span, error.message.clone()))
                    .collect();
                let actual: Vec<_> = resolution
                    .errors
                    .iter()
                    .map(|error| (error.span, error.message.clone()))
                    .collect();
                assert_eq!(expected, actual, "{}", path.display());
            }
        }
    }
}

fn assert_no_internal_error(text: &str, context: &str) {
    for finding in ns_lsp::analysis::analyze(text).findings {
        assert!(
            !finding.message.contains("internal error"),
            "{context}: {}",
            finding.message
        );
    }
}

#[test]
fn no_module_produces_an_internal_error() {
    for path in modules() {
        assert_no_internal_error(&read(&path), &path.display().to_string());
    }
}

/// Every prefix of a module ending at a token boundary, for the goldens and
/// the kernel; for the corpus modules, which are far larger, 64 prefixes
/// spread evenly over the file. Every prefix of every corpus module is
/// quadratic in its size and would make this test take minutes in a debug
/// build; 64 cut points per module still cut inside every kind of
/// construct the corpus holds.
#[test]
fn no_truncated_module_produces_an_internal_error() {
    for path in modules() {
        let text = read(&path);
        let (tokens, _) = ns_syntax::lexer::Lexer::new(&text).run();
        let boundaries: Vec<usize> = tokens.iter().map(|token| token.span.end as usize).collect();
        let in_corpus = path
            .components()
            .any(|component| component.as_os_str() == "corpus");
        let stride = if in_corpus {
            (boundaries.len() / 64).max(1)
        } else {
            1
        };
        for end in boundaries.iter().step_by(stride) {
            let context = format!("{} cut at byte {end}", path.display());
            assert_no_internal_error(&text[..*end], &context);
        }
    }
}
