//! `ns-next run`.

use std::path::Path;
use std::process::ExitCode;

use crate::entry;
use crate::source;

/// `ns-next run`: parse, resolve, bootstrap, load,
/// instantiate the root class with whatever its declared primary factory
/// takes, send the entry selector with whatever `entry_arguments` sizes for it,
/// report.
pub(crate) fn execute(
    file: &Path,
    send: &str,
    lowering: ns_runtime::Lowering,
    cache_statistics: bool,
    program_arguments: &[String],
) -> ExitCode {
    let loaded = match source::load_source(file) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let (mut interpreter, class) = source::bootstrap(&loaded, lowering);
    let instance = match entry::instantiate_root(&mut interpreter, &class) {
        Ok(instance) => instance,
        Err(code) => return code,
    };
    let platform = interpreter.platform();
    let arguments =
        match entry::entry_arguments(&mut interpreter, send, &platform, program_arguments) {
            Ok(arguments) => arguments,
            Err(message) => {
                eprintln!("error: {message}");
                return ExitCode::FAILURE;
            }
        };
    let selector = ns_runtime::Symbol::intern(send);
    let result = interpreter.send(&instance, selector, &arguments);
    // To stderr, after the answer is decided: no fixture or golden reads
    // stdout's exit status or stdout content any differently for it.
    if cache_statistics {
        entry::print_cache_statistics(&interpreter.cache_statistics());
    }
    entry::report_entry_send(result)
}
