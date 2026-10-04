//! Reading, parsing and resolving a module -- the pipeline every command
//! that runs, benchmarks, disassembles or builds a `.ns` file shares
//! before an interpreter exists.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ns_syntax::{Diagnostic, Severity};

/// A module read, parsed, and resolved -- everything `run`, `disasm`, and
/// `build` share before an interpreter exists.
pub struct LoadedSource {
    pub source: String,
    pub origin: ns_runtime::SourceOrigin,
    pub resolved: ns_runtime::ResolvedModule,
}

/// Reads, parses, and resolves `file`, reporting diagnostics the way every
/// command does. `Err` carries the exit code to return.
pub fn load_source(file: &Path) -> Result<LoadedSource, ExitCode> {
    let source = match std::fs::read_to_string(file) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("error: reading {}: {error}", file.display());
            return Err(ExitCode::FAILURE);
        }
    };
    let module = match ns_syntax::parse(&source) {
        Ok(module) => module,
        Err(diagnostics) => {
            print_diagnostics(file, &source, &diagnostics);
            return Err(ExitCode::FAILURE);
        }
    };
    let origin = ns_runtime::SourceOrigin::File(file.to_path_buf());
    let resolved = match ns_runtime::resolve(module, source.as_str().into(), origin.clone()) {
        Ok(resolved) => resolved,
        Err(errors) => {
            for error in errors {
                eprintln!("resolve error: {}", error.message);
            }
            return Err(ExitCode::FAILURE);
        }
    };
    Ok(LoadedSource {
        source,
        origin,
        resolved,
    })
}

/// Parses and resolves a source compiled into the binary -- the manifest --
/// exactly as a file's source, under an embedded origin. A shipped source
/// that does not parse is a build defect of this binary, not input.
pub fn load_embedded(name: &str, source: &str) -> LoadedSource {
    let module = ns_syntax::parse(source)
        .unwrap_or_else(|_| panic!("internal: the embedded {name} does not parse"));
    let origin = ns_runtime::SourceOrigin::Embedded(name.to_string());
    let resolved = ns_runtime::resolve(module, source.into(), origin.clone())
        .unwrap_or_else(|_| panic!("internal: the embedded {name} does not resolve"));
    LoadedSource {
        source: source.to_string(),
        origin,
        resolved,
    }
}

pub fn collect_ns_files(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_ns_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "ns") {
            out.push(path);
        }
    }
}

pub fn print_diagnostics(file: &Path, source: &str, diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        let (line, column) = line_column(source, diagnostic.span.start);
        let level = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        eprintln!(
            "{}:{line}:{column}: {level}: {}",
            file.display(),
            diagnostic.message
        );
    }
}

fn line_column(source: &str, offset: u32) -> (usize, usize) {
    let offset = offset as usize;
    let mut line = 1;
    let mut column = 1;
    for (byte_offset, character) in source.char_indices() {
        if byte_offset >= offset {
            break;
        }
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}
