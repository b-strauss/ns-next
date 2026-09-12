//! `ns-next run`.

use std::path::Path;
use std::process::ExitCode;

use crate::entry;
use crate::source;

/// `ns-next run`: parse, resolve, bootstrap, load,
/// instantiate the root class with whatever its declared primary factory
/// takes, send the entry selector with whatever `entry_args` sizes for it,
/// report.
pub(crate) fn execute(file: &Path, send: &str, lowering: ns_runtime::Lowering) -> ExitCode {
    let loaded = match source::load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let (mut interp, class) = source::bootstrap(&loaded, lowering);
    let instance = match entry::instantiate_root(&mut interp, class) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let platform = interp.platform();
    let args = match entry::entry_args(send, platform) {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let selector = ns_runtime::Symbol::intern(send);
    entry::report_entry_send(interp.send(instance, selector, args))
}
