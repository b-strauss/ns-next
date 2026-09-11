//! Reading, parsing and resolving a module -- the pipeline every command
//! that runs, disassembles, snapshots, loads or builds a `.ns` file shares
//! before an interpreter exists.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ns_syntax::{Diagnostic, Severity};

/// A module read, parsed, and resolved -- everything `run`, `disasm`, and
/// `snapshot` share before an interpreter exists.
pub(crate) struct LoadedSource {
    pub(crate) source: String,
    pub(crate) origin: ns_runtime::SourceOrigin,
    pub(crate) resolved: ns_runtime::ResolvedModule,
}

/// Reads, parses, and resolves `file`, reporting diagnostics exactly as
/// the Milestone 1 `run` did. `Err` carries the exit code to return.
pub(crate) fn load_source(file: &Path) -> Result<LoadedSource, ExitCode> {
    let source = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: reading {}: {e}", file.display());
            return Err(ExitCode::FAILURE);
        }
    };
    let module = match ns_syntax::parse(&source) {
        Ok(m) => m,
        Err(diags) => {
            print_diagnostics(file, &source, &diags);
            return Err(ExitCode::FAILURE);
        }
    };
    let origin = ns_runtime::SourceOrigin::File(file.to_path_buf());
    let resolved = match ns_runtime::resolve(module, source.as_str().into(), origin.clone()) {
        Ok(r) => r,
        Err(errors) => {
            for e in errors {
                eprintln!("resolve error: {}", e.message);
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

/// Bootstraps an interpreter and loads the module's root class, which
/// compiles every body onto the declaration it belongs to -- before anything
/// is instantiated, so the root class's instance initializer runs compiled
/// too.
pub(crate) fn bootstrap(loaded: &LoadedSource) -> (ns_runtime::Interpreter, ns_runtime::ClassRef) {
    let mut interp = ns_runtime::Interpreter::bootstrap();
    let class = interp.load_module(&loaded.resolved);
    (interp, class)
}

pub(crate) fn collect_ns_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_ns_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "ns") {
            out.push(path);
        }
    }
}

pub(crate) fn print_diagnostics(file: &Path, src: &str, diags: &[Diagnostic]) {
    for d in diags {
        let (line, col) = line_col(src, d.span.start);
        let level = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        eprintln!("{}:{line}:{col}: {level}: {}", file.display(), d.message);
    }
}

fn line_col(src: &str, offset: u32) -> (usize, usize) {
    let offset = offset as usize;
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in src.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}
