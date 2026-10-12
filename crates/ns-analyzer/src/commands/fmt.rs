//! `ns-next-analyzer fmt`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ns_tooling::source::{collect_ns_files, print_diagnostics};

/// Formats every file in `paths` (a directory stands for the `.ns` files
/// below it; none stands for the current directory). With `check`, nothing
/// is written and each file that would change is listed on stdout.
///
/// One bad file never stops the run: it is reported, left as it was, and
/// the status says that something failed.
pub(crate) fn execute(paths: &[PathBuf], check: bool) -> ExitCode {
    let mut files = Vec::new();
    if paths.is_empty() {
        collect_ns_files(Path::new("."), &mut files);
    }
    for path in paths {
        if path.is_dir() {
            collect_ns_files(path, &mut files);
        } else {
            files.push(path.clone());
        }
    }
    files.sort();
    files.dedup();

    let mut succeeded = true;
    for file in &files {
        succeeded &= format_file(file, check);
    }
    if succeeded {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Whether `file` is formatted afterwards (or, with `check`, already is).
fn format_file(file: &Path, check: bool) -> bool {
    let source = match std::fs::read_to_string(file) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("error: cannot read {}: {error}", file.display());
            return false;
        }
    };
    let formatted = match ns_syntax::format(&source) {
        Ok(formatted) => formatted,
        Err(diagnostics) => {
            print_diagnostics(file, &source, &diagnostics);
            return false;
        }
    };
    if formatted == source {
        return true;
    }
    if check {
        println!("{}", file.display());
        return false;
    }
    match write_replacing(file, &formatted) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("error: cannot write {}: {error}", file.display());
            false
        }
    }
}

/// Writes beside the target and renames over it, so that a failure part way
/// never leaves a truncated source behind.
fn write_replacing(file: &Path, text: &str) -> std::io::Result<()> {
    let mut temporary_name = file.file_name().unwrap_or_default().to_os_string();
    temporary_name.push(".fmt-temporary");
    let temporary = file.with_file_name(temporary_name);
    let result = std::fs::write(&temporary, text).and_then(|()| std::fs::rename(&temporary, file));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
