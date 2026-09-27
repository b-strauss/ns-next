//! `ns-next load`.

use std::path::Path;
use std::process::ExitCode;

/// `ns-next load`: deserialize an artifact and start it. What "start" means
/// depends on which of the two shapes it is, and the header says which.
///
/// A **stateful image** already carries its instance, so a `--send` is sent
/// to the root it decoded, and without one the command reports what it loaded
/// and stops.
///
/// A **stateless binary** carries no instance at all. Its root class is
/// instantiated here, against this interpreter's own freshly bootstrapped
/// platform, and then the selector it recorded is sent -- `--send` overrides
/// that selector but not the instantiation. Running the factory and every
/// initializer at load is what makes the artifact stateless in the first
/// place: it has never held a capability.
///
/// Either way the entry send goes through the same `entry_arguments` convention
/// and the same `report_entry_send` as `run`, so `load` of an artifact prints
/// exactly what `run` of its source prints and nothing else. A malformed
/// artifact is a message, never a stack trace.
pub(crate) fn execute(
    image: &Path,
    send: Option<&str>,
    print_source: bool,
    program_arguments: &[String],
) -> ExitCode {
    let bytes = match std::fs::read(image) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: reading {}: {error}", image.display());
            return ExitCode::FAILURE;
        }
    };
    let ns_runtime::image::LoadedImage {
        mut interpreter,
        root_class,
        roots,
        origin: _,
        source,
        entry,
        stateless,
    } = match ns_runtime::image::load(&bytes) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("error: loading {}: {error}", image.display());
            return ExitCode::FAILURE;
        }
    };
    if print_source {
        let Some(source) = source else {
            eprintln!(
                "error: {} carries no source (it was not written with --debug-source)",
                image.display()
            );
            return ExitCode::FAILURE;
        };
        print!("{source}");
        return ExitCode::SUCCESS;
    }
    // `stateless` (the decoded header flag), not `roots.is_empty()`, decides
    // whether to instantiate: it says what the artifact *is*, where an empty
    // `roots` is only what a binary happens to look like. `image::snapshot`
    // is a library API that a caller can hand an empty `roots` slice to,
    // producing a valid *stateful* image that carries zero root values --
    // and that image must still be treated as stateful here, since
    // instantiating it would re-run a primary factory and every instance
    // initializer in a world where they already ran, minting a second,
    // unrelated instance in place of the (absent) one the artifact
    // deliberately carried. A decoded `stateless: true` with a non-empty
    // `roots` would mean `image::load` built an inconsistent `LoadedImage`
    // -- a decoder bug, not a case this branch tolerates.
    let instance = if stateless {
        // A binary carries no instance: the primary factory and every
        // initializer run here, against this interpreter's freshly
        // bootstrapped platform, so capabilities enter at start time rather
        // than having been baked in at build time. Instantiating in the CLI
        // rather than in `image::load` keeps an existing boundary: deciding
        // *that* a command-line entry point supplies the platform is a CLI
        // concern -- nothing in the runtime needs to know it was invoked
        // from a terminal.
        match crate::entry::instantiate_root(&mut interpreter, &root_class) {
            Ok(instance) => instance,
            Err(code) => return code,
        }
    } else {
        // Pre-existing behaviour for a stateful image, unconditional on
        // `roots.len()`: no `--send` means report the root count and stop,
        // sending nothing -- including a stateful image with zero roots,
        // which reports "0 root value(s)" rather than being mistaken for a
        // binary.
        let Some(_) = send else {
            println!("loaded {}: {} root value(s)", image.display(), roots.len());
            return ExitCode::SUCCESS;
        };
        let Some(instance) = roots.into_iter().next() else {
            eprintln!("error: image has no root instance to send to");
            return ExitCode::FAILURE;
        };
        instance
    };
    // `--send` overrides whatever the artifact recorded. In practice the
    // `None` arm below is unreachable today: a binary always records an
    // entry (`image::load` refuses a stateless artifact without one), and a
    // stateful image with neither already returned above, in the `else`
    // (stateful) branch. It stays as a real check rather than an
    // `unreachable!()` because that pairing is an invariant of the image
    // format, not of this function -- a future image that records an entry
    // too should fall through to sending it, not panic.
    let selector = match send.or_else(|| entry.map(ns_runtime::Symbol::as_str)) {
        Some(selector) => selector,
        None => {
            eprintln!("error: this artifact records no entry point; pass --send");
            return ExitCode::FAILURE;
        }
    };
    let platform = interpreter.platform();
    let arguments = match crate::entry::entry_arguments(
        &mut interpreter,
        selector,
        &platform,
        program_arguments,
    ) {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };
    crate::entry::report_entry_send(interpreter.send(
        &instance,
        ns_runtime::Symbol::intern(selector),
        &arguments,
    ))
}
