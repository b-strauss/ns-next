//! `ns-next load`.

use std::path::Path;
use std::process::ExitCode;

use crate::debug::{self, DebugOptions, HostStart};
use crate::program;

/// `ns-next load`: decode an image `build` wrote and start it -- send its
/// one root `main: platform args:` with this interpreter's platform, which
/// is the first the application ever sees. An image that was not packaged
/// for a start (an `image::snapshot` a host wrote) is refused, since there
/// is nothing to start. A malformed image is a message, never a panic.
/// Under a debugger (`debug`), as `run`: `main:args:` is sent once the
/// client configured the session, and the client hears the exit status.
pub(crate) fn execute(
    image: &Path,
    print_source: bool,
    debug: Option<DebugOptions>,
    program_arguments: &[String],
) -> ExitCode {
    let bytes = match std::fs::read(image) {
        Ok(bytes) => bytes,
        Err(error) => {
            report_line!("error: reading {}: {error}", image.display());
            return ExitCode::FAILURE;
        }
    };
    let mut loaded = match ns_runtime::image::load(&bytes) {
        Ok(loaded) => loaded,
        Err(error) => {
            report_line!("error: loading {}: {error}", image.display());
            return ExitCode::FAILURE;
        }
    };
    if print_source {
        // The header's one flag says whether every module carries its source
        // or none does, so a module without one means the image has none.
        if loaded.modules.iter().any(|module| module.source.is_none()) {
            report_line!(
                "error: {} carries no source (it was written with --strip-source)",
                image.display()
            );
            return ExitCode::FAILURE;
        }
        for module in &loaded.modules {
            if let Err(code) =
                crate::entry::print_output(module.source.as_deref().unwrap_or_default())
            {
                return code;
            }
        }
        return ExitCode::SUCCESS;
    }
    if !loaded.start || loaded.roots.len() != 1 {
        report_line!(
            "error: {} is not a started image: `build` writes one, with the application as its \
             only root",
            image.display()
        );
        return ExitCode::FAILURE;
    }
    crate::entry::report_jit_unavailable(&loaded.interpreter);
    let application = loaded.roots[0].clone();
    // The manifest's requests `main:args:` makes are warned of as `run`
    // warns of them; those of the assembly `build` warned of.
    let mut kept = program::kept_manifests(&mut loaded);
    let Some(options) = debug else {
        let result = crate::entry::send_main(
            &mut loaded.interpreter,
            &application,
            program_arguments,
            None,
        );
        program::warn_of_kept_ambiguities(&mut loaded.interpreter, &mut kept);
        return crate::entry::report_entry_send(result);
    };
    // An image's modules have no files: a stop shows each by a reference.
    // Each is named by the id and version the image recorded, as `run`
    // names the program's modules.
    let modules = loaded
        .modules
        .iter()
        .map(|module| debug::ProgramModule {
            module: ns_runtime::ModuleName::of_identity(&module.identity),
            path: None,
            identity: None,
            text: module.source.as_deref().map(str::to_string),
        })
        .collect();
    let link = match debug::start(
        &options,
        &mut loaded.interpreter,
        image.display().to_string(),
        modules,
    ) {
        Ok(link) => link,
        Err(code) => return code,
    };
    let code = if link.wait_for_configuration(&mut loaded.interpreter) == HostStart::Terminate
        || link.terminate_requested()
    {
        debug::report_terminated()
    } else {
        let result = crate::entry::send_main(
            &mut loaded.interpreter,
            &application,
            program_arguments,
            Some(&link),
        );
        program::warn_of_kept_ambiguities(&mut loaded.interpreter, &mut kept);
        crate::entry::report_entry_send(result)
    };
    link.exited(debug::status_of(code));
    code
}
