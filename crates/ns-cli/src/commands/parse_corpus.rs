//! `ns-next parse-corpus`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::source::collect_ns_files;

/// Parses every `.ns` file under `directory`. Every one must parse: the
/// corpus is real Newspeak, and the parser follows the specification's
/// grammar, so a file that fails is a parser defect or a program the
/// specification refuses -- either way something to decide, not to list
/// away.
pub(crate) fn execute(directory: &Path) -> ExitCode {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_ns_files(directory, &mut files);
    files.sort();

    let (mut parsed, mut failed) = (0, 0);
    for file in &files {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        let source = std::fs::read_to_string(file).unwrap_or_default();
        if ns_syntax::parse(&source).is_ok() {
            parsed += 1;
        } else {
            failed += 1;
            eprintln!("FAIL {name}");
        }
    }
    println!("parsed {parsed}, failed {failed}");
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
