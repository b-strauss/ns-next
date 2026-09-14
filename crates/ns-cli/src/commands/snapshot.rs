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
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let (mut interpreter, class) = source::bootstrap(&loaded, lowering);
    let instance = match entry::instantiate_root(&mut interpreter, &class) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let platform = interpreter.platform();
    let mut roots: Vec<ns_runtime::Root> = vec![instance.clone()];
    if let Some(selector) = send {
        let arguments = match entry::entry_arguments(&interpreter, selector, &platform) {
            Ok(arguments) => arguments,
            Err(message) => {
                eprintln!("error: {message}");
                return ExitCode::FAILURE;
            }
        };
        match interpreter.send(&instance, ns_runtime::Symbol::intern(selector), &arguments) {
            Ok(result) => roots.push(result),
            Err(signal) => return entry::report_entry_send(Err(signal)),
        }
    }
    // Fetched only now, right before encoding: encoding runs no code, so
    // nothing can move these values between here and the write.
    let bytes = match interpreter.enter(|mutator| {
        let platform = mutator.fetch(&platform);
        let root_values: Vec<ns_runtime::Value> =
            roots.iter().map(|root| mutator.fetch(root)).collect();
        ns_runtime::image::snapshot(
            mutator,
            debug_source.then_some(loaded.source.as_str()),
            &loaded.origin,
            entry::class_reference(mutator, &class),
            &platform,
            &root_values,
        )
    }) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: snapshot failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    match std::fs::write(out, bytes) {
        Ok(()) => {
            println!("wrote {}", out.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: writing {}: {error}", out.display());
            ExitCode::FAILURE
        }
    }
}
