//! `ns-next run`.

use std::path::Path;
use std::process::ExitCode;

use crate::{entry, program};

/// `ns-next run`: read the program, compile every module, assemble the
/// application, send it `main: platform args:`, report.
pub(crate) fn execute(
    file: &Path,
    lowering: ns_runtime::Lowering,
    cache_statistics: bool,
    script: Option<&str>,
    program_arguments: &[String],
) -> ExitCode {
    let program = match program::read_program(file) {
        Ok(program) => program,
        Err(code) => return code,
    };
    let arguments = match program::entry_arguments(&program, script, program_arguments) {
        Ok(arguments) => arguments,
        Err(code) => return code,
    };
    let mut loaded = program::load_program(&program, lowering);
    let application = match program::instantiate_application(&program, &mut loaded) {
        Ok(application) => application,
        Err(code) => return code,
    };
    let result = entry::send_main(&mut loaded.interpreter, &application, &arguments);
    // To stderr, after the answer is decided: no fixture or golden reads
    // stdout any differently for it.
    if cache_statistics {
        entry::print_cache_statistics(&loaded.interpreter.cache_statistics());
    }
    entry::report_entry_send(result)
}
