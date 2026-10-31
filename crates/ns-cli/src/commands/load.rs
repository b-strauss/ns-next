//! `ns-next load`.

use std::path::Path;
use std::process::ExitCode;

/// `ns-next load`: decode an image `build` wrote and start it -- send its
/// one root `main: platform args:` with this interpreter's platform, which
/// is the first the application ever sees. An image that was not packaged
/// for a start (an `image::snapshot` a host wrote) is refused, since there
/// is nothing to start. A malformed image is a message, never a panic.
pub(crate) fn execute(image: &Path, print_source: bool, program_arguments: &[String]) -> ExitCode {
    let bytes = match std::fs::read(image) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("error: reading {}: {error}", image.display());
            return ExitCode::FAILURE;
        }
    };
    let mut loaded = match ns_runtime::image::load(&bytes) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("error: loading {}: {error}", image.display());
            return ExitCode::FAILURE;
        }
    };
    if print_source {
        // The header's one flag says whether every module carries its source
        // or none does, so a module without one means the image has none.
        if loaded.modules.iter().any(|module| module.source.is_none()) {
            eprintln!(
                "error: {} carries no source (it was written with --strip-source)",
                image.display()
            );
            return ExitCode::FAILURE;
        }
        for module in &loaded.modules {
            print!("{}", module.source.as_deref().unwrap_or_default());
        }
        return ExitCode::SUCCESS;
    }
    if !loaded.start || loaded.roots.len() != 1 {
        eprintln!(
            "error: {} is not a started image: `build` writes one, with the application as its \
             only root",
            image.display()
        );
        return ExitCode::FAILURE;
    }
    crate::entry::report_jit_unavailable(&loaded.interpreter);
    let application = loaded.roots[0].clone();
    crate::entry::report_entry_send(crate::entry::send_main(
        &mut loaded.interpreter,
        &application,
        program_arguments,
    ))
}
