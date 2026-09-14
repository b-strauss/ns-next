//! `ns-next run`.

use std::path::Path;
use std::process::ExitCode;

use crate::entry;
use crate::source;

/// `ns-next run`: parse, resolve, bootstrap, load,
/// instantiate the root class with whatever its declared primary factory
/// takes, send the entry selector with whatever `entry_arguments` sizes for it,
/// report.
pub(crate) fn execute(file: &Path, send: &str, lowering: ns_runtime::Lowering) -> ExitCode {
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
    let arguments = match entry::entry_arguments(&interpreter, send, &platform) {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };
    let selector = ns_runtime::Symbol::intern(send);
    entry::report_entry_send(interpreter.send(&instance, selector, &arguments))
}
