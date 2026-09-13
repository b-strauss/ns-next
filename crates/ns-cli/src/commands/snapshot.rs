//! `ns-next snapshot`.

use std::path::Path;
use std::process::ExitCode;

use crate::entry;
use crate::source;

/// `ns-next snapshot`: `run`'s pipeline up to (and optionally including)
/// the entry send, then a serialized image of the resulting world. The
/// image's own code table is what a later `load` runs.
pub(crate) fn execute(
    file: &Path,
    out: &Path,
    send: Option<&str>,
    debug_source: bool,
    lowering: ns_runtime::Lowering,
) -> ExitCode {
    let loaded = match source::load_source(file) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let (mut interp, class) = source::bootstrap(&loaded, lowering);
    let instance = match entry::instantiate_root(&mut interp, &class) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let platform = interp.platform();
    let mut roots: Vec<ns_runtime::Root> = vec![instance.clone()];
    if let Some(selector) = send {
        let args = match entry::entry_args(&interp, selector, &platform) {
            Ok(args) => args,
            Err(msg) => {
                eprintln!("error: {msg}");
                return ExitCode::FAILURE;
            }
        };
        match interp.send(&instance, ns_runtime::Symbol::intern(selector), &args) {
            Ok(result) => roots.push(result),
            Err(signal) => return entry::report_entry_send(Err(signal)),
        }
    }
    // Fetched only now, right before encoding: encoding runs no code, so
    // nothing can move these values between here and the write.
    let platform = interp.fetch(&platform);
    let root_values: Vec<ns_runtime::Value> = roots.iter().map(|r| interp.fetch(r)).collect();
    let bytes = match ns_runtime::image::snapshot(
        &interp,
        debug_source.then_some(loaded.source.as_str()),
        &loaded.origin,
        entry::class_ref(&interp, &class),
        &platform,
        &root_values,
    ) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: snapshot failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    match std::fs::write(out, bytes) {
        Ok(()) => {
            println!("wrote {}", out.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: writing {}: {e}", out.display());
            ExitCode::FAILURE
        }
    }
}
