//! `ns-next parse`.

use std::path::Path;
use std::process::ExitCode;

use crate::source::print_diagnostics;

pub(crate) fn execute(file: &Path, ast: bool) -> ExitCode {
    let source = match std::fs::read_to_string(file) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("{}: error: {error}", file.display());
            return ExitCode::FAILURE;
        }
    };
    match ns_syntax::parse(&source) {
        Ok(module) => {
            if ast {
                println!("{module:#?}");
            }
            println!("OK {}", file.display());
            ExitCode::SUCCESS
        }
        Err(diagnostics) => {
            print_diagnostics(file, &source, &diagnostics);
            ExitCode::FAILURE
        }
    }
}
